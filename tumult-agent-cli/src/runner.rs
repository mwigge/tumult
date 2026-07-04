//! Blocking subprocess executor for [`CliInvocation`]s.
//!
//! These are one-shot batch calls that run to completion, so plain
//! [`std::process`] is the right tool — no async runtime. Timeout handling
//! polls `try_wait` (~50 ms) and kills + reaps the child on expiry, the same
//! pattern used by `tumult-cli`'s exec activity runner.

use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::adapter::{snippet, AgentCliAdapter, CliInvocation, PromptRequest, RawOutput};
use crate::error::AgentCliError;

const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Detect, build, run, and parse a prompt through `adapter` end to end.
///
/// This is the high-level entry point: it probes the CLI first so missing
/// binaries and known-bad auth fail fast with actionable hints, then executes
/// the one-shot invocation and extracts the model answer.
///
/// # Errors
///
/// - [`AgentCliError::BinaryNotFound`] when the probe finds no binary.
/// - [`AgentCliError::NotAuthenticated`] when the probe definitively reports
///   the CLI as logged out.
/// - [`AgentCliError::Timeout`] when the subprocess exceeds `req.timeout`.
/// - [`AgentCliError::InvocationFailed`] on spawn failure, non-zero exit, or
///   an in-band error result.
/// - [`AgentCliError::OutputParse`] when the output shape is unusable.
pub fn run_prompt(
    adapter: &dyn AgentCliAdapter,
    req: &PromptRequest,
) -> Result<String, AgentCliError> {
    let probe = adapter.detect();
    if !probe.installed {
        return Err(AgentCliError::BinaryNotFound {
            name: adapter.name().to_string(),
            install_hint: adapter.install_hint().to_string(),
        });
    }
    if probe.logged_in == Some(false) {
        return Err(AgentCliError::NotAuthenticated {
            name: adapter.name().to_string(),
            auth_hint: adapter.auth_hint().to_string(),
        });
    }

    let invocation = adapter.build_invocation(req)?;
    let raw = run(adapter.name(), &invocation)?;
    if raw.exit_code != Some(0) {
        return Err(AgentCliError::InvocationFailed {
            name: adapter.name().to_string(),
            explain: adapter.explain_failure(&raw),
        });
    }
    adapter.parse_output(&raw)
}

/// Execute a single non-interactive [`CliInvocation`] and capture its output.
///
/// `NO_COLOR=1` is always set on the child (overridable by
/// [`CliInvocation::env`]) so output stays machine-parseable. Stdin is piped
/// when [`CliInvocation::stdin`] is `Some`, closed otherwise. On timeout the
/// child is killed and reaped so it neither keeps running nor becomes a
/// zombie.
///
/// # Errors
///
/// - [`AgentCliError::InvocationFailed`] when the argv is empty, the process
///   cannot be spawned, or waiting on it fails.
/// - [`AgentCliError::Timeout`] when the deadline expires.
///
/// A non-zero exit is *not* an error at this layer; it is reported through
/// [`RawOutput::exit_code`] so callers can consult `explain_failure`.
pub fn run(adapter_name: &str, invocation: &CliInvocation) -> Result<RawOutput, AgentCliError> {
    let Some((program, args)) = invocation.argv.split_first() else {
        return Err(AgentCliError::InvocationFailed {
            name: adapter_name.to_string(),
            explain: "invocation argv is empty".to_string(),
        });
    };

    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(&invocation.cwd)
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if invocation.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    for (key, value) in &invocation.env {
        command.env(key, value);
    }

    tracing::debug!(
        adapter = adapter_name,
        program,
        "spawning agent CLI subprocess"
    );
    let mut child = command
        .spawn()
        .map_err(|e| AgentCliError::InvocationFailed {
            name: adapter_name.to_string(),
            explain: format!("failed to spawn `{program}`: {e}"),
        })?;

    // Feed stdin from a separate thread so a child that never reads it (or a
    // prompt larger than the pipe buffer) cannot deadlock the poll loop.
    if let Some(input) = invocation.stdin.clone() {
        if let Some(mut stdin) = child.stdin.take() {
            thread::spawn(move || {
                // EPIPE from an early-exiting child is expected; ignore it.
                let _ = stdin.write_all(input.as_bytes());
            });
        }
    }
    let stdout_reader = child.stdout.take().map(spawn_reader);
    let stderr_reader = child.stderr.take().map(spawn_reader);

    let deadline = Instant::now() + invocation.timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                let now = Instant::now();
                if now >= deadline {
                    // Kill and reap the child so it doesn't keep running (or
                    // become a zombie) after we return.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(AgentCliError::Timeout {
                        name: adapter_name.to_string(),
                        seconds: invocation.timeout.as_secs_f64(),
                    });
                }
                thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(now)));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(AgentCliError::InvocationFailed {
                    name: adapter_name.to_string(),
                    explain: format!("failed to wait on `{program}`: {e}"),
                });
            }
        }
    };

    Ok(RawOutput {
        stdout: join_reader(stdout_reader),
        stderr: join_reader(stderr_reader),
        exit_code: status.code(),
    })
}

/// Run `<bin> --version` and return trimmed stdout, or a human-readable
/// reason the probe failed.
pub(crate) fn probe_version(
    adapter_name: &str,
    bin: &Path,
    timeout: Duration,
) -> Result<String, String> {
    let invocation = CliInvocation {
        argv: vec![bin.display().to_string(), "--version".to_string()],
        stdin: None,
        cwd: std::env::temp_dir(),
        env: Vec::new(),
        timeout,
    };
    match run(adapter_name, &invocation) {
        Ok(raw) if raw.exit_code == Some(0) => Ok(raw.stdout.trim().to_string()),
        Ok(raw) => Err(explain_cli_failure(
            &format!("{} --version", bin.display()),
            &raw,
        )),
        Err(e) => Err(e.to_string()),
    }
}

/// Extract the first `x.y.z` (digits-only) version token from probe output.
pub(crate) fn extract_semver(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|token| {
        let token = token.trim_start_matches('v');
        let parts: Vec<&str> = token.split('.').collect();
        let numeric = |s: &&str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
        if parts.len() >= 3 && parts.iter().take(3).all(numeric) {
            Some(parts[..3].join("."))
        } else {
            None
        }
    })
}

/// Shared human-readable failure text for a finished subprocess.
pub(crate) fn explain_cli_failure(label: &str, raw: &RawOutput) -> String {
    let status = raw.exit_code.map_or_else(
        || "terminated by signal".to_string(),
        |code| format!("exit code {code}"),
    );
    let stderr = raw.stderr.trim();
    let detail = if stderr.is_empty() {
        raw.stdout.trim()
    } else {
        stderr
    };
    if detail.is_empty() {
        format!("`{label}` failed ({status}) with no output")
    } else {
        format!("`{label}` failed ({status}): {}", snippet(detail, 500))
    }
}

fn spawn_reader<R: Read + Send + 'static>(mut source: R) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = source.read_to_end(&mut buf);
        buf
    })
}

fn join_reader(handle: Option<thread::JoinHandle<Vec<u8>>>) -> String {
    handle
        .and_then(|h| h.join().ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{explain_cli_failure, extract_semver};
    use crate::adapter::RawOutput;

    #[test]
    fn extract_semver_finds_plain_version() {
        assert_eq!(
            extract_semver("2.0.13 (Claude Code)").as_deref(),
            Some("2.0.13")
        );
        assert_eq!(
            extract_semver("codex-cli 0.46.0").as_deref(),
            Some("0.46.0")
        );
        assert_eq!(extract_semver("v1.2.3-beta.1").as_deref(), None);
        assert_eq!(extract_semver("no version here"), None);
    }

    #[test]
    fn explain_prefers_stderr_over_stdout() {
        let raw = RawOutput {
            stdout: "partial".to_string(),
            stderr: "boom".to_string(),
            exit_code: Some(2),
        };
        let text = explain_cli_failure("claude -p", &raw);
        assert!(text.contains("exit code 2"), "{text}");
        assert!(text.contains("boom"), "{text}");
        assert!(!text.contains("partial"), "{text}");
    }

    #[test]
    fn explain_reports_signal_termination() {
        let raw = RawOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: None,
        };
        let text = explain_cli_failure("codex exec", &raw);
        assert!(text.contains("terminated by signal"), "{text}");
        assert!(text.contains("no output"), "{text}");
    }
}
