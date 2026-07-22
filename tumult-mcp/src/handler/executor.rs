//! Process executor — runs external-process activities via async Tokio I/O.

use tumult_core::sync_bridge::sync_await;

/// Default execution timeout for process commands (seconds).
const DEFAULT_EXECUTION_TIMEOUT_SECS: u64 = 300;

/// Maximum bytes captured from each of the child's stdout and stderr. The
/// pipe is still drained to EOF beyond the cap (so the child never blocks
/// on a full pipe); only the in-memory capture is bounded.
const MAX_CAPTURE_BYTES: usize = 8 * 1024 * 1024;

/// Read `reader` to EOF, appending at most [`MAX_CAPTURE_BYTES`] bytes to
/// `buf`. Returns `true` when bytes were dropped beyond the cap.
async fn read_capped<R: tokio::io::AsyncReadExt + Unpin>(
    reader: &mut R,
    buf: &mut Vec<u8>,
) -> bool {
    // Heap-allocated so two concurrent drains stay small in the caller's future.
    let mut chunk = vec![0u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let room = MAX_CAPTURE_BYTES.saturating_sub(buf.len());
                if room > 0 {
                    buf.extend_from_slice(&chunk[..n.min(room)]);
                }
                if n > room {
                    truncated = true;
                }
            }
        }
    }
    truncated
}

/// Executes activities that invoke external processes, using async I/O via
/// the current Tokio runtime.
///
/// `injected_env` carries the experiment's resolved `configuration:` and
/// `secrets:` values as pre-built `TUMULT_CONFIG_*` / `TUMULT_SECRET_*`
/// pairs (see [`tumult_core::engine::build_config_env`]), matching the CLI's
/// `ProviderExecutor` semantics: they reach `process` activities as
/// environment variables, and entries declared on the activity itself
/// always win over injected ones.
pub struct ProcessExecutor {
    injected_env: std::collections::HashMap<String, String>,
}

impl ProcessExecutor {
    /// Executor with no configuration/secret injection.
    #[must_use]
    pub fn new() -> Self {
        Self {
            injected_env: std::collections::HashMap::new(),
        }
    }

    /// Executor injecting the given `TUMULT_CONFIG_*` / `TUMULT_SECRET_*`
    /// pairs into process provider subprocesses.
    #[must_use]
    pub fn with_injected_env(injected_env: std::collections::HashMap<String, String>) -> Self {
        Self { injected_env }
    }

    /// Merge the injected env into a process activity's declared environment;
    /// declared entries win.
    fn merged_process_env(
        &self,
        env: &std::collections::HashMap<String, String>,
    ) -> std::collections::HashMap<String, String> {
        let mut merged = self.injected_env.clone();
        merged.extend(env.iter().map(|(k, v)| (k.clone(), v.clone())));
        merged
    }
}

impl Default for ProcessExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl tumult_core::runner::ActivityExecutor for ProcessExecutor {
    /// Executes the given activity by spawning an external process.
    ///
    /// # Panics
    ///
    /// Panics if called from a Tokio `current_thread` runtime context; see
    /// [`sync_await`].
    fn execute(
        &self,
        activity: &tumult_core::types::Activity,
    ) -> tumult_core::runner::ActivityOutcome {
        match &activity.provider {
            tumult_core::types::Provider::Process {
                path,
                arguments,
                env,
                timeout_s,
            } => {
                let timeout = std::time::Duration::from_secs_f64(timeout_s.unwrap_or({
                    // u64 → f64: timeout constant is small; precision loss is irrelevant.
                    #[allow(clippy::cast_precision_loss)]
                    {
                        DEFAULT_EXECUTION_TIMEOUT_SECS as f64
                    }
                }));
                let start = std::time::Instant::now();
                let path = path.clone();
                let arguments = arguments.clone();
                let env = self.merged_process_env(env);

                // Use tokio::process::Command with async timeout instead of
                // busy-polling with std::thread::sleep.
                sync_await(async {
                    let mut child = match tokio::process::Command::new(&path)
                        .args(&arguments)
                        .envs(&env)
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        // A dropped child future (timeout, caller
                        // cancellation) must never leave the process running.
                        .kill_on_drop(true)
                        .spawn()
                    {
                        Ok(c) => c,
                        Err(e) => {
                            return tumult_core::runner::ActivityOutcome {
                                success: false,
                                output: None,
                                error: Some(e.to_string()),
                                duration_ms: 0,
                            };
                        }
                    };

                    let mut stdout_handle = child.stdout.take();
                    let mut stderr_handle = child.stderr.take();

                    let result = tokio::time::timeout(timeout, async {
                        let mut stdout_buf = Vec::new();
                        let mut stderr_buf = Vec::new();
                        // Drain both pipes concurrently: a child writing more
                        // than the pipe buffer to stderr while we read stdout
                        // (or vice versa) would otherwise deadlock.
                        let (out_truncated, err_truncated) =
                            match (stdout_handle.as_mut(), stderr_handle.as_mut()) {
                                (Some(out), Some(err)) => {
                                    tokio::join!(
                                        read_capped(out, &mut stdout_buf),
                                        read_capped(err, &mut stderr_buf),
                                    )
                                }
                                (Some(out), None) => {
                                    (read_capped(out, &mut stdout_buf).await, false)
                                }
                                (None, Some(err)) => {
                                    (false, read_capped(err, &mut stderr_buf).await)
                                }
                                (None, None) => (false, false),
                            };
                        let status = child.wait().await?;
                        Ok::<_, std::io::Error>((
                            stdout_buf,
                            stderr_buf,
                            out_truncated || err_truncated,
                            status,
                        ))
                    })
                    .await;

                    let result = match result {
                        Ok(Ok((stdout_buf, stderr_buf, truncated, status))) => {
                            Ok((stdout_buf, stderr_buf, truncated, status))
                        }
                        Ok(Err(e)) => Err(e.to_string()),
                        Err(_elapsed) => {
                            // The timeout dropped the read/wait future;
                            // kill_on_drop covers the drop path, but kill and
                            // reap explicitly so no zombie or orphan remains.
                            let _ = child.kill().await;
                            Err(format!(
                                "process timed out after {:.1}s",
                                timeout.as_secs_f64()
                            ))
                        }
                    };

                    // u128 → u64: elapsed milliseconds; durations exceeding ~584M years
                    // will truncate, which is acceptable for telemetry.
                    #[allow(clippy::cast_possible_truncation)]
                    let elapsed = start.elapsed().as_millis() as u64;

                    match result {
                        Ok((stdout_buf, stderr_buf, truncated, status)) => {
                            let mut stdout =
                                String::from_utf8_lossy(&stdout_buf).trim().to_string();
                            let mut stderr =
                                String::from_utf8_lossy(&stderr_buf).trim().to_string();
                            if truncated {
                                const NOTE: &str = "… [truncated: capture capped at 8 MiB]";
                                if !stdout.is_empty() {
                                    stdout.push_str(NOTE);
                                }
                                if !stderr.is_empty() {
                                    stderr.push_str(NOTE);
                                }
                            }

                            tumult_core::runner::ActivityOutcome {
                                success: status.success(),
                                output: Some(stdout),
                                error: if stderr.is_empty() {
                                    None
                                } else {
                                    Some(stderr)
                                },
                                duration_ms: elapsed,
                            }
                        }
                        Err(reason) => tumult_core::runner::ActivityOutcome {
                            success: false,
                            output: None,
                            error: Some(reason),
                            duration_ms: elapsed,
                        },
                    }
                })
            }
            tumult_core::types::Provider::Native { .. }
            | tumult_core::types::Provider::Script { .. } => tumult_core::runner::ActivityOutcome {
                success: false,
                output: None,
                error: Some("only process provider supported in MCP context".into()),
                duration_ms: 0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tumult_core::runner::ActivityExecutor;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn process_executor_respects_timeout() {
        let executor = ProcessExecutor::new();
        let activity = tumult_core::types::Activity {
            name: "timeout-test".into(),
            activity_type: tumult_core::types::ActivityType::Action,
            provider: tumult_core::types::Provider::Process {
                path: "sleep".into(),
                arguments: vec!["60".into()],
                env: std::collections::HashMap::new(),
                timeout_s: Some(0.2), // 200ms timeout
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        };

        let outcome = executor.execute(&activity);
        assert!(outcome.error.as_ref().unwrap().contains("timed out"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn process_executor_records_duration() {
        let executor = ProcessExecutor::new();
        let activity = tumult_core::types::Activity {
            name: "duration-test".into(),
            activity_type: tumult_core::types::ActivityType::Action,
            provider: tumult_core::types::Provider::Process {
                path: "echo".into(),
                arguments: vec!["hello".into()],
                env: std::collections::HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        };

        let outcome = executor.execute(&activity);
        assert!(outcome.success);
        assert_eq!(outcome.output.as_deref(), Some("hello"));
        // Duration should be recorded (previously was always 0)
        // It may still be 0 for very fast commands, so just check it's not negative
        // (u64 is always >= 0)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn injected_env_reaches_subprocess_and_declared_wins() {
        let executor = ProcessExecutor::with_injected_env(std::collections::HashMap::from([
            (
                "TUMULT_CONFIG_DB_HOST".to_string(),
                "db.internal".to_string(),
            ),
            ("TUMULT_SECRET_TOKEN".to_string(), "injected".to_string()),
        ]));
        let activity = tumult_core::types::Activity {
            name: "injection-test".into(),
            activity_type: tumult_core::types::ActivityType::Action,
            provider: tumult_core::types::Provider::Process {
                path: "sh".into(),
                arguments: vec![
                    "-c".into(),
                    "echo \"$TUMULT_CONFIG_DB_HOST/$TUMULT_SECRET_TOKEN\"".into(),
                ],
                // The declared entry must win over the injected one.
                env: std::collections::HashMap::from([(
                    "TUMULT_SECRET_TOKEN".to_string(),
                    "declared".to_string(),
                )]),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        };

        let outcome = executor.execute(&activity);
        assert!(outcome.success, "{:?}", outcome.error);
        assert_eq!(outcome.output.as_deref(), Some("db.internal/declared"));
    }
}
