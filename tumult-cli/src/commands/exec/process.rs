use tumult_core::runner::ActivityOutcome;

/// Execute an external process with optional timeout, using async I/O when a
/// Tokio runtime is available or falling back to `std::process::Command`.
///
/// # Panics
///
/// Panics if a Tokio runtime is present but it uses the `current_thread`
/// scheduler. `tokio::task::block_in_place` requires the `multi_thread`
/// scheduler and will panic otherwise.
pub(super) fn execute_process(
    path: &str,
    arguments: &[String],
    env: &std::collections::HashMap<String, String>,
    timeout_s: Option<&f64>,
) -> ActivityOutcome {
    // Background activities run on std::thread::scope threads without a Tokio
    // runtime.  Detect this and fall back to std::process::Command.
    if tokio::runtime::Handle::try_current().is_err() {
        return execute_process_sync(path, arguments, env, timeout_s);
    }

    let start = std::time::Instant::now();

    let path = path.to_string();
    let arguments = arguments.to_vec();
    let env = env.clone();
    let timeout_dur = timeout_s.map(|s| std::time::Duration::from_secs_f64(*s));

    tokio::task::block_in_place(move || {
        tokio::runtime::Handle::current().block_on(async {
            let mut cmd = tokio::process::Command::new(&path);
            cmd.args(&arguments);
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            for (k, v) in &env {
                cmd.env(k, v);
            }

            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(e) => {
                    return ActivityOutcome {
                        success: false,
                        output: None,
                        error: Some(format!("failed to execute '{path}': {e}")),
                        // u128 → u64: elapsed ms; truncation only possible after ~584M years.
                        #[allow(clippy::cast_possible_truncation)]
                        duration_ms: start.elapsed().as_millis() as u64,
                    };
                }
            };

            let result = if let Some(dur) = timeout_dur {
                match tokio::time::timeout(dur, child.wait()).await {
                    Ok(Ok(status)) => {
                        let stdout = {
                            let mut buf = Vec::new();
                            if let Some(mut out) = child.stdout.take() {
                                use tokio::io::AsyncReadExt;
                                let _ = out.read_to_end(&mut buf).await;
                            }
                            buf
                        };
                        let stderr = {
                            let mut buf = Vec::new();
                            if let Some(mut err) = child.stderr.take() {
                                use tokio::io::AsyncReadExt;
                                let _ = err.read_to_end(&mut buf).await;
                            }
                            buf
                        };
                        Ok(std::process::Output {
                            status,
                            stdout,
                            stderr,
                        })
                    }
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(_elapsed) => {
                        let _ = child.kill().await;
                        Err("timed out".to_string())
                    }
                }
            } else {
                child.wait_with_output().await.map_err(|e| e.to_string())
            };

            // u128 → u64: elapsed ms; truncation only possible after ~584M years.
            #[allow(clippy::cast_possible_truncation)]
            let duration_ms = start.elapsed().as_millis() as u64;

            match result {
                Ok(output) => {
                    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

                    ActivityOutcome {
                        success: output.status.success(),
                        output: if stdout.is_empty() {
                            None
                        } else {
                            Some(stdout)
                        },
                        error: if stderr.is_empty() {
                            None
                        } else {
                            Some(stderr)
                        },
                        duration_ms,
                    }
                }
                Err(reason) => ActivityOutcome {
                    success: false,
                    output: None,
                    error: Some(format!("process '{path}' {reason}")),
                    duration_ms,
                },
            }
        })
    })
}

/// Synchronous process execution for background threads (no Tokio runtime).
fn execute_process_sync(
    path: &str,
    arguments: &[String],
    env: &std::collections::HashMap<String, String>,
    timeout_s: Option<&f64>,
) -> ActivityOutcome {
    let start = std::time::Instant::now();

    let mut cmd = std::process::Command::new(path);
    cmd.args(arguments);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return ActivityOutcome {
                success: false,
                output: None,
                error: Some(format!("failed to execute '{path}': {e}")),
                #[allow(clippy::cast_possible_truncation)]
                duration_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    let result = if let Some(&secs) = timeout_s {
        let dur = std::time::Duration::from_secs_f64(secs);
        let deadline = std::time::Instant::now() + dur;
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
        loop {
            match child.try_wait() {
                Ok(Some(_status)) => break child.wait_with_output().map_err(|e| e.to_string()),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        // Timeout — kill and reap the child so it doesn't keep
                        // running (or become a zombie) after we return.
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err(format!("process '{path}' timed out"));
                    }
                    std::thread::sleep(
                        POLL_INTERVAL
                            .min(deadline.saturating_duration_since(std::time::Instant::now())),
                    );
                }
                Err(e) => break Err(e.to_string()),
            }
        }
    } else {
        child.wait_with_output().map_err(|e| e.to_string())
    };

    #[allow(clippy::cast_possible_truncation)]
    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let success = output.status.success();
            ActivityOutcome {
                success,
                output: if stdout.is_empty() {
                    None
                } else {
                    Some(stdout)
                },
                error: if success {
                    if stderr.is_empty() {
                        None
                    } else {
                        Some(stderr)
                    }
                } else {
                    Some(if stderr.is_empty() {
                        format!("process '{path}' exited with {}", output.status)
                    } else {
                        stderr
                    })
                },
                duration_ms,
            }
        }
        Err(reason) => ActivityOutcome {
            success: false,
            output: None,
            error: Some(reason),
            duration_ms,
        },
    }
}
