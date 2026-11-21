use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;
use tumult_core::runner::ActivityOutcome;
use tumult_core::sync_bridge::sync_await;

use super::{fired, timeout_of};

/// Per-stream capture limit. Excess bytes are drained to avoid pipe deadlock.
const OUTPUT_CAP: usize = 8 * 1024 * 1024;
const TRUNCATION_NOTE: &str = "[output truncated at 8 MiB]";

/// The reason in the error of a process that was ended by a stop.
const STOPPED: &str = "stopped before it finished";

/// Execute a process with one timeout covering both exit and output draining.
/// The shared bridge also supports the runner's plain background threads.
/// On Unix, a timeout or a `stop` that fires kills the process group,
/// including descendants whose parent already exited. Pipe readers are
/// cancelled rather than detached.
pub(super) fn execute_process(
    path: &str,
    arguments: &[String],
    env: &std::collections::HashMap<String, String>,
    timeout_s: Option<&f64>,
    stop: Option<&CancellationToken>,
) -> ActivityOutcome {
    let start = std::time::Instant::now();
    let timeout = match timeout_of(timeout_s) {
        Ok(timeout) => timeout,
        Err(reason) => return failure(path, &reason, start),
    };
    sync_await(async {
        let mut command = tokio::process::Command::new(path);
        command
            .args(arguments)
            .envs(env)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = match tumult_core::spawn::alone(|| command.spawn()) {
            Ok(child) => child,
            Err(error) => return failure(path, &format!("failed to execute: {error}"), start),
        };
        // Keep the group identity even after wait() reaps the direct child.
        #[cfg(unix)]
        let process_group = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let completion = Box::pin(async {
            tokio::try_join!(child.wait(), read_optional(stdout), read_optional(stderr))
        });
        let within_timeout = async {
            match timeout {
                Some(duration) => match tokio::time::timeout(duration, completion).await {
                    Ok(result) => result,
                    Err(_) => Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "timed out",
                    )),
                },
                None => completion.await,
            }
        };
        let result = tokio::select! {
            result = within_timeout => result,
            () = fired(stop) => Err(std::io::Error::other(STOPPED)),
        };
        let (status, stdout, stderr) = match result {
            Ok(result) => result,
            Err(error) => {
                #[cfg(unix)]
                if let Some(pid) = process_group {
                    kill_process_group(pid);
                }
                #[cfg(not(unix))]
                let _ = child.start_kill();
                // The child may already have exited while descendants held its pipes.
                if let Err(reap_error) = child.wait().await {
                    return failure(path, &format!("{error}; reap failed: {reap_error}"), start);
                }
                return failure(path, &error.to_string(), start);
            }
        };
        let stdout = lossy_trimmed(&stdout.0, stdout.1);
        let stderr = lossy_trimmed(&stderr.0, stderr.1);
        ActivityOutcome {
            success: status.success(),
            output: (!stdout.is_empty()).then_some(stdout),
            error: if !stderr.is_empty() {
                Some(stderr)
            } else if !status.success() {
                Some(format!("process '{path}' exited with {status}"))
            } else {
                None
            },
            duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        }
    })
}

fn failure(path: &str, reason: &str, start: std::time::Instant) -> ActivityOutcome {
    ActivityOutcome {
        success: false,
        output: None,
        error: Some(format!("process '{path}' {reason}")),
        duration_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

async fn read_optional<R: tokio::io::AsyncRead + Unpin>(
    reader: Option<R>,
) -> std::io::Result<(Vec<u8>, bool)> {
    let Some(mut reader) = reader else {
        return Ok((Vec::new(), false));
    };
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            return Ok((bytes, truncated));
        }
        let remaining = OUTPUT_CAP.saturating_sub(bytes.len());
        bytes.extend_from_slice(&chunk[..count.min(remaining)]);
        truncated |= count > remaining;
    }
}

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

#[cfg(unix)]
fn kill_process_group(pid: u32) {
    let Ok(group_id) = i32::try_from(pid) else {
        return;
    };
    // Safety: the child was spawned with process_group(0), making its pid the
    // group id. ESRCH is expected if the whole group already exited.
    unsafe {
        libc::kill(-group_id, libc::SIGKILL);
    }
}
