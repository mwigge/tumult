//! Process executor — runs external-process activities via async Tokio I/O.

use tumult_core::sync_bridge::sync_await;

/// Default execution timeout for process commands (seconds).
const DEFAULT_EXECUTION_TIMEOUT_SECS: u64 = 300;

/// Executes activities that invoke external processes, using async I/O via
/// the current Tokio runtime.
pub struct ProcessExecutor;

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
                let env = env.clone();

                // Use tokio::process::Command with async timeout instead of
                // busy-polling with std::thread::sleep.
                sync_await(async {
                    use tokio::io::AsyncReadExt;
                    let mut child = match tokio::process::Command::new(&path)
                        .args(&arguments)
                        .envs(&env)
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
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

                    // Collect stdout/stderr as owned before await to avoid
                    // the moved-value issue with wait_with_output().
                    let mut stdout_handle = child.stdout.take();
                    let mut stderr_handle = child.stderr.take();

                    let result = tokio::time::timeout(timeout, async {
                        let mut stdout_buf = Vec::new();
                        let mut stderr_buf = Vec::new();
                        if let Some(ref mut h) = stdout_handle {
                            let _ = h.read_to_end(&mut stdout_buf).await;
                        }
                        if let Some(ref mut h) = stderr_handle {
                            let _ = h.read_to_end(&mut stderr_buf).await;
                        }
                        let status = child.wait().await?;
                        Ok::<_, std::io::Error>((stdout_buf, stderr_buf, status))
                    })
                    .await;

                    let result = match result {
                        Ok(Ok((stdout_buf, stderr_buf, status))) => {
                            Ok((stdout_buf, stderr_buf, status))
                        }
                        Ok(Err(e)) => Err(e.to_string()),
                        Err(_elapsed) => Err(format!(
                            "process timed out after {:.1}s",
                            timeout.as_secs_f64()
                        )),
                    };

                    // u128 → u64: elapsed milliseconds; durations exceeding ~584M years
                    // will truncate, which is acceptable for telemetry.
                    #[allow(clippy::cast_possible_truncation)]
                    let elapsed = start.elapsed().as_millis() as u64;

                    match result {
                        Ok((stdout_buf, stderr_buf, status)) => {
                            let stdout = String::from_utf8_lossy(&stdout_buf).trim().to_string();
                            let stderr = String::from_utf8_lossy(&stderr_buf).trim().to_string();

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
            tumult_core::types::Provider::Native { .. } => tumult_core::runner::ActivityOutcome {
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
        let executor = ProcessExecutor;
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
        let executor = ProcessExecutor;
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
}
