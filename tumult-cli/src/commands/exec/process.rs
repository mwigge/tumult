use tumult_core::runner::ActivityOutcome;
use tumult_core::sync_bridge::sync_await;

/// Per-stream cap on captured child output. Output beyond the cap is drained
/// and discarded (so the child never blocks on a full pipe) and the captured
/// text is annotated with a truncation note.
const OUTPUT_CAP: usize = 8 * 1024 * 1024;

const TRUNCATION_NOTE: &str = "[output truncated at 8 MiB]";

/// Execute an external process with optional timeout, using async I/O when a
/// Tokio runtime is available or falling back to `std::process::Command`.
///
/// Both paths drain piped stdout/stderr concurrently with waiting, so a child
/// emitting more than the OS pipe buffer (~64 KiB) cannot deadlock on write
/// and surface as a false timeout. On timeout the whole process group is
/// killed (Unix), so grandchildren spawned by a wrapper script (e.g.
/// `stress-ng`) don't survive; elsewhere only the direct child is killed.
///
/// # Panics
///
/// Panics if a Tokio runtime is present but it uses the `current_thread`
/// scheduler; see [`sync_await`].
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

    sync_await(async {
        let mut cmd = tokio::process::Command::new(&path);
        cmd.args(&arguments);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        // Put the child in its own process group so a timeout can kill the
        // whole group (including grandchildren) rather than just the child.
        #[cfg(unix)]
        cmd.process_group(0);
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

        // Drain the pipes concurrently with waiting: a child whose output
        // exceeds the OS pipe buffer would otherwise block on write and be
        // killed as a false timeout.
        let stdout_task = child
            .stdout
            .take()
            .map(|out| tokio::spawn(read_bounded_async(out)));
        let stderr_task = child
            .stderr
            .take()
            .map(|err| tokio::spawn(read_bounded_async(err)));

        let status_result = if let Some(dur) = timeout_dur {
            match tokio::time::timeout(dur, child.wait()).await {
                Ok(Ok(status)) => Ok(status),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_elapsed) => {
                    kill_timed_out_child_async(&mut child).await;
                    Err("timed out".to_string())
                }
            }
        } else {
            child.wait().await.map_err(|e| e.to_string())
        };

        // The reader tasks finish at EOF once the child has exited.
        let (stdout, stdout_truncated) = collect_async(stdout_task).await;
        let (stderr, stderr_truncated) = collect_async(stderr_task).await;

        // u128 → u64: elapsed ms; truncation only possible after ~584M years.
        #[allow(clippy::cast_possible_truncation)]
        let duration_ms = start.elapsed().as_millis() as u64;

        match status_result {
            Ok(status) => {
                let stdout = lossy_trimmed(&stdout, stdout_truncated);
                let stderr = lossy_trimmed(&stderr, stderr_truncated);

                ActivityOutcome {
                    success: status.success(),
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
}

/// Synchronous process execution for background threads (no Tokio runtime).
fn execute_process_sync(
    path: &str,
    arguments: &[String],
    env: &std::collections::HashMap<String, String>,
    timeout_s: Option<&f64>,
) -> ActivityOutcome {
    const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
    let start = std::time::Instant::now();

    let mut cmd = std::process::Command::new(path);
    cmd.args(arguments);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    // Put the child in its own process group so a timeout can kill the whole
    // group (including grandchildren) rather than just the child.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
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

    // Drain the pipes on reader threads so a verbose child can't block on
    // write and show up as a false timeout.
    let stdout_thread = child
        .stdout
        .take()
        .map(|out| std::thread::spawn(move || read_bounded_sync(out)));
    let stderr_thread = child
        .stderr
        .take()
        .map(|err| std::thread::spawn(move || read_bounded_sync(err)));

    let status_result = if let Some(&secs) = timeout_s {
        let dur = std::time::Duration::from_secs_f64(secs);
        let deadline = std::time::Instant::now() + dur;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        // Timeout — kill the child's process group (Unix) or
                        // the child itself, then reap it so it doesn't keep
                        // running (or become a zombie) after we return.
                        kill_timed_out_child_sync(&mut child);
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
        child.wait().map_err(|e| e.to_string())
    };

    let (stdout, stdout_truncated) = collect_sync(stdout_thread);
    let (stderr, stderr_truncated) = collect_sync(stderr_thread);

    #[allow(clippy::cast_possible_truncation)]
    let duration_ms = start.elapsed().as_millis() as u64;

    match status_result {
        Ok(status) => {
            let stdout = lossy_trimmed(&stdout, stdout_truncated);
            let stderr = lossy_trimmed(&stderr, stderr_truncated);
            let success = status.success();
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
                        format!("process '{path}' exited with {status}")
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

/// Read a stream to EOF, keeping at most [`OUTPUT_CAP`] bytes. The stream is
/// always drained to EOF — even past the cap — so the writer never blocks on
/// a full pipe. Returns the captured bytes and whether truncation occurred.
async fn read_bounded_async<R: tokio::io::AsyncRead + Unpin>(mut reader: R) -> (Vec<u8>, bool) {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let remaining = OUTPUT_CAP.saturating_sub(buf.len());
                buf.extend_from_slice(&chunk[..n.min(remaining)]);
                if n > remaining {
                    truncated = true;
                }
            }
        }
    }
    (buf, truncated)
}

/// Blocking counterpart of [`read_bounded_async`] for reader threads.
fn read_bounded_sync<R: std::io::Read>(mut reader: R) -> (Vec<u8>, bool) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let remaining = OUTPUT_CAP.saturating_sub(buf.len());
                buf.extend_from_slice(&chunk[..n.min(remaining)]);
                if n > remaining {
                    truncated = true;
                }
            }
        }
    }
    (buf, truncated)
}

/// Collect a reader task's output, treating a join failure as empty output.
async fn collect_async(task: Option<tokio::task::JoinHandle<(Vec<u8>, bool)>>) -> (Vec<u8>, bool) {
    match task {
        Some(handle) => handle.await.unwrap_or_default(),
        None => (Vec::new(), false),
    }
}

/// Collect a reader thread's output, treating a join failure as empty output.
fn collect_sync(thread: Option<std::thread::JoinHandle<(Vec<u8>, bool)>>) -> (Vec<u8>, bool) {
    match thread {
        Some(handle) => handle.join().unwrap_or_default(),
        None => (Vec::new(), false),
    }
}

/// Decode captured bytes lossily, trimming whitespace and appending a
/// truncation note when the stream exceeded the capture cap.
fn lossy_trimmed(bytes: &[u8], truncated: bool) -> String {
    let mut text = String::from_utf8_lossy(bytes).trim().to_string();
    if truncated {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(TRUNCATION_NOTE);
    }
    text
}

/// Kill the timed-out child's whole process group on Unix, so grandchildren
/// (e.g. `stress-ng` spawned by a wrapper script) don't survive; fall back to
/// killing only the direct child elsewhere. Always reaps the child.
#[cfg(unix)]
async fn kill_timed_out_child_async(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        kill_process_group(pid);
    }
    let _ = child.wait().await;
}

/// Non-Unix counterpart of [`kill_timed_out_child_async`].
#[cfg(not(unix))]
async fn kill_timed_out_child_async(child: &mut tokio::process::Child) {
    let _ = child.kill().await;
}

/// Synchronous counterpart of [`kill_timed_out_child_async`].
#[cfg(unix)]
fn kill_timed_out_child_sync(child: &mut std::process::Child) {
    kill_process_group(child.id());
    let _ = child.wait();
}

/// Non-Unix counterpart of [`kill_timed_out_child_sync`].
#[cfg(not(unix))]
fn kill_timed_out_child_sync(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Send `SIGKILL` to the process group whose id is `pid` (children are
/// spawned with `process_group(0)`, so the group id equals the child pid).
#[cfg(unix)]
fn kill_process_group(pid: u32) {
    let Ok(group_id) = i32::try_from(pid) else {
        return;
    };
    // Safety: `kill` with a negative pid targets the process group; errors
    // (e.g. ESRCH when the group already exited) are safely ignorable.
    unsafe {
        libc::kill(-group_id, libc::SIGKILL);
    }
}
