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

use tumult_otel::logging::err;

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
        name: "agent_cli.process.spawning",
        {
            agent.adapter = adapter_name,
            process.executable.name = program.as_str(),
        },
        "spawning agent CLI subprocess"
    );
    let mut child =
        tumult_spawn::alone(|| command.spawn()).map_err(|e| AgentCliError::InvocationFailed {
            name: adapter_name.to_string(),
            explain: format!("failed to spawn `{program}`: {e}"),
        })?;

    // Feed stdin from a separate thread so a child that never reads it (or a
    // prompt larger than the pipe buffer) cannot deadlock the poll loop.
    if let Some(input) = invocation.stdin.clone() {
        if let Some(mut stdin) = child.stdin.take() {
            thread::spawn(move || {
                if let Err(error) = stdin.write_all(input.as_bytes()) {
                    log_stdin_failure(&error);
                }
            });
        }
    }
    let stdout_reader = child.stdout.take().map(|out| spawn_reader(out, "stdout"));
    let stderr_reader = child.stderr.take().map(|err| spawn_reader(err, "stderr"));

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
        stdout: join_reader(stdout_reader, "stdout"),
        stderr: join_reader(stderr_reader, "stderr"),
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

/// The prompt could not be written to the child's stdin. A child that exits
/// before reading it closes the pipe, which is expected and logged at debug;
/// anything else means the prompt may not have been delivered.
fn log_stdin_failure(error: &std::io::Error) {
    if error.kind() == std::io::ErrorKind::BrokenPipe {
        tracing::debug!(
            name: "agent_cli.stdin.closed_early",
            "agent CLI closed stdin before reading the whole prompt"
        );
        return;
    }
    tracing::warn!(
        name: "agent_cli.stdin.write_failed",
        { error.type = "io", error.message = %err(error) },
        "prompt may not have been delivered to the agent CLI"
    );
}

fn spawn_reader<R: Read + Send + 'static>(
    source: R,
    stream: &'static str,
) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || read_all(source, stream))
}

/// Reads `source` to EOF. A read error keeps what arrived before it and is
/// logged, so truncated output is never mistaken for the whole output.
fn read_all<R: Read>(mut source: R, stream: &'static str) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Err(error) = source.read_to_end(&mut buf) {
        tracing::warn!(
            name: "agent_cli.output.read_failed",
            {
                process.stream = stream,
                bytes_read = buf.len(),
                error.type = "io",
                error.message = %err(&error),
            },
            "agent CLI output truncated by a read error"
        );
    }
    buf
}

fn join_reader(handle: Option<thread::JoinHandle<Vec<u8>>>, stream: &'static str) -> String {
    let Some(handle) = handle else {
        return String::new();
    };
    let Ok(bytes) = handle.join() else {
        tracing::warn!(
            name: "agent_cli.output.reader_panicked",
            { process.stream = stream },
            "agent CLI output lost, its reader thread panicked"
        );
        return String::new();
    };
    String::from_utf8_lossy(&bytes).into_owned()
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

    /// A reader that yields some bytes, then fails.
    struct FailingReader {
        served: bool,
    }

    impl std::io::Read for FailingReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.served {
                return Err(std::io::Error::other("pipe torn down"));
            }
            self.served = true;
            buf[..7].copy_from_slice(b"partial");
            Ok(7)
        }
    }

    #[test]
    fn a_failed_output_read_keeps_what_arrived_and_is_logged() {
        let mut bytes = Vec::new();
        let logs = tumult_otel::logging::testing::capture_json(tracing::Level::DEBUG, || {
            bytes = super::read_all(FailingReader { served: false }, "stderr");
        });

        assert_eq!(bytes, b"partial");
        let event = &logs.named("agent_cli.output.read_failed")[0];
        assert_eq!(event["level"], "WARN");
        assert_eq!(event["process.stream"], "stderr");
        assert_eq!(event["bytes_read"], 7);
        assert_eq!(event["error.type"], "io");
        assert_eq!(event["error.message"], "pipe torn down");
    }

    #[test]
    fn a_complete_output_read_logs_nothing() {
        let logs = tumult_otel::logging::testing::capture_json(tracing::Level::DEBUG, || {
            assert_eq!(super::read_all(&b"all of it"[..], "stdout"), b"all of it");
        });
        assert!(logs.events.is_empty(), "{}", logs.raw);
    }

    #[test]
    fn an_undelivered_prompt_is_a_warning_unless_the_child_just_left() {
        let logs = tumult_otel::logging::testing::capture_json(tracing::Level::DEBUG, || {
            super::log_stdin_failure(&std::io::Error::other("no space left on device"));
            super::log_stdin_failure(&std::io::Error::from(std::io::ErrorKind::BrokenPipe));
        });

        let failed = logs.named("agent_cli.stdin.write_failed");
        assert_eq!(failed.len(), 1, "{}", logs.raw);
        assert_eq!(failed[0]["level"], "WARN");
        assert_eq!(failed[0]["error.message"], "no space left on device");
        let closed = logs.named("agent_cli.stdin.closed_early");
        assert_eq!(closed.len(), 1, "{}", logs.raw);
        assert_eq!(closed[0]["level"], "DEBUG");
    }

    #[test]
    fn spawning_an_agent_cli_is_a_named_debug_event() {
        let invocation = crate::adapter::CliInvocation {
            argv: vec!["true".to_string()],
            cwd: std::env::temp_dir(),
            env: Vec::new(),
            stdin: None,
            timeout: std::time::Duration::from_secs(10),
        };
        let logs = tumult_otel::logging::testing::capture_json(tracing::Level::DEBUG, || {
            super::run("claude", &invocation).unwrap();
        });
        let event = &logs.named("agent_cli.process.spawning")[0];
        assert_eq!(event["level"], "DEBUG");
        assert_eq!(event["agent.adapter"], "claude");
        assert_eq!(event["process.executable.name"], "true");
    }
}
