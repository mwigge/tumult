//! Script plugin executor — runs scripts with TUMULT_* env vars.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum ExecutorError {
    #[error("script not found: {0}")]
    ScriptNotFound(String),
    #[error("script execution failed: {0}")]
    ExecutionFailed(#[from] std::io::Error),
    #[error("script timed out after {0}s")]
    Timeout(f64),
    #[error("script stopped before it finished")]
    Stopped,
    #[error("null byte in script argument key or value: {0}")]
    NullByteInArgument(String),
    #[error(
        "script plugins are not supported on Windows (actions run through /bin/sh); \
         use a native or process provider instead"
    )]
    UnsupportedPlatform,
    #[error(
        "invalid argument name '{0}': it is exported as a TUMULT_* env var, so it must match \
         [A-Za-z_][A-Za-z0-9_]* after uppercasing"
    )]
    InvalidArgumentName(String),
    #[error(
        "conflicting argument names '{first}' and '{second}': both become TUMULT_{uppercased} — \
         rename one"
    )]
    ConflictingArgumentNames {
        first: String,
        second: String,
        uppercased: String,
    },
}

/// Exit status of a completed script process.
///
/// Distinguishes between a normal exit code (`Code`) and termination by
/// an OS signal without a numeric code (`Signal`).  The magic sentinel
/// value `-1` that `std::process::ExitStatus::code()` returns on signal
/// termination is replaced by this typed variant so callers can match
/// exhaustively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptExitStatus {
    /// Process exited with the given numeric code.
    Code(i32),
    /// Process was terminated by an OS signal (no numeric exit code).
    Signal,
}

impl ScriptExitStatus {
    /// Returns the numeric exit code, or `None` if terminated by a signal.
    #[must_use]
    pub fn code(self) -> Option<i32> {
        match self {
            Self::Code(n) => Some(n),
            Self::Signal => None,
        }
    }

    /// Returns `true` only when the process exited with code `0`.
    #[must_use]
    pub fn is_success(self) -> bool {
        matches!(self, Self::Code(0))
    }
}

/// Result of executing a script plugin action or probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptResult {
    pub exit_status: ScriptExitStatus,
    /// What the script wrote to stdout, up to the capture limit. Nothing is
    /// ever added to it: output that was cut is flagged in
    /// [`stdout_truncated`](Self::stdout_truncated).
    pub stdout: String,
    pub stderr: String,
    /// Whether stdout held more than the capture limit and was cut.
    pub stdout_truncated: bool,
    /// Whether stderr held more than the capture limit and was cut.
    pub stderr_truncated: bool,
}

impl ScriptResult {
    /// Returns `true` only when the script exited with code `0`.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.exit_status.is_success()
    }
}

/// Validate that no argument keys or values contain null bytes or empty keys.
///
/// Null bytes in environment variable names or values can cause truncation
/// or injection issues in child processes. Empty keys produce no-op env vars
/// with the `TUMULT_` prefix that silently swallow caller mistakes.
///
/// # Errors
///
/// Returns [`ExecutorError::NullByteInArgument`] if any key or value contains a
/// null byte (`\0`), or if any key is empty.
#[must_use = "callers must handle null-byte validation errors"]
pub fn validate_arguments<S: std::hash::BuildHasher>(
    arguments: &HashMap<String, String, S>,
) -> Result<(), ExecutorError> {
    for (k, v) in arguments {
        if k.is_empty() {
            return Err(ExecutorError::NullByteInArgument("<empty key>".to_string()));
        }
        if k.contains('\0') || v.contains('\0') {
            return Err(ExecutorError::NullByteInArgument(k.clone()));
        }
    }
    Ok(())
}

/// Build the TUMULT_* environment variables from a key-value argument map.
///
/// Keys become env var names after uppercasing, so each key must form a
/// valid shell identifier (`[A-Za-z_][A-Za-z0-9_]*`); two keys that
/// uppercase to the same name (`foo` vs `FOO`) would collide, and which one
/// survived would depend on random `HashMap` iteration order — they are
/// rejected with a clear error instead of silently overwriting.
///
/// # Errors
///
/// Returns [`ExecutorError::InvalidArgumentName`] if a key is not a valid
/// shell identifier after uppercasing.
/// Returns [`ExecutorError::ConflictingArgumentNames`] if two keys uppercase
/// to the same env var name.
pub fn build_env_vars<S: std::hash::BuildHasher>(
    arguments: &HashMap<String, String, S>,
) -> Result<HashMap<String, String>, ExecutorError> {
    let mut env = HashMap::with_capacity(arguments.len());
    let mut originals: HashMap<String, &str> = HashMap::with_capacity(arguments.len());
    for (k, v) in arguments {
        let uppercased = k.to_uppercase();
        if !is_valid_env_identifier(&uppercased) {
            return Err(ExecutorError::InvalidArgumentName(k.clone()));
        }
        if let Some(&first) = originals.get(&uppercased) {
            return Err(ExecutorError::ConflictingArgumentNames {
                first: first.to_string(),
                second: k.clone(),
                uppercased,
            });
        }
        originals.insert(uppercased.clone(), k);
        env.insert(format!("TUMULT_{uppercased}"), v.clone());
    }
    Ok(env)
}

/// Valid POSIX environment variable name: `[A-Za-z_][A-Za-z0-9_]*`.
fn is_valid_env_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Execute a script at the given path with TUMULT_* env vars, ended early
/// when `stop` completes.
///
/// `plugin_root` is the canonical directory the plugin was loaded from.
/// The script path is resolved relative to `plugin_root` and must remain
/// within it after canonicalization, preventing path-traversal via manifests
/// that specify `../../etc/passwd` as the script path.
///
/// The shell runs in its own process group: on timeout the whole group is
/// killed (Unix), so children the script spawned don't survive it. A stop
/// kills the process group of the script, like a timeout does, so
/// children the script started do not outlive it.
///
/// # Errors
///
/// Returns [`ExecutorError::ScriptNotFound`] if the script path does not exist.
/// Returns [`ExecutorError::ScriptNotFound`] if the resolved path escapes `plugin_root`.
/// Returns [`ExecutorError::NullByteInArgument`] if any argument contains a null byte or empty key.
/// Returns [`ExecutorError::InvalidArgumentName`] if any argument key is not a valid
/// shell identifier after uppercasing.
/// Returns [`ExecutorError::ConflictingArgumentNames`] if two keys uppercase to the same name.
/// Returns [`ExecutorError::ExecutionFailed`] if the process cannot be spawned.
/// Returns [`ExecutorError::Timeout`] if the script does not finish within the given duration.
/// Returns [`ExecutorError::Stopped`] if `stop` completes before the script
/// ends.
#[cfg(not(windows))]
pub async fn execute_script_until<S: std::hash::BuildHasher>(
    script_path: &Path,
    plugin_root: &Path,
    arguments: &HashMap<String, String, S>,
    timeout: Option<Duration>,
    stop: impl std::future::Future<Output = ()>,
) -> Result<ScriptResult, ExecutorError> {
    // Pre-compute the display string once to avoid repeated allocations
    // (PLUGIN-ALLOC-01: was called 3-4× via .display().to_string() inline).
    let path_str = script_path.display().to_string();
    let timeout_f64 = timeout.map(|d| d.as_secs_f64());
    let _span = crate::telemetry::begin_execute(&path_str, timeout_f64);
    crate::telemetry::event_script_started(&path_str);

    if !script_path.exists() {
        return Err(ExecutorError::ScriptNotFound(path_str));
    }

    // Bounds-check: resolve the script path and verify it stays within the
    // plugin root directory (PLUGIN-SEC-01). This prevents a manifest with
    // `script: ../../etc/passwd` from reaching outside the plugin directory.
    let canonical_root = std::fs::canonicalize(plugin_root)
        .map_err(|_| ExecutorError::ScriptNotFound(path_str.clone()))?;
    let canonical_script = std::fs::canonicalize(script_path)
        .map_err(|_| ExecutorError::ScriptNotFound(path_str.clone()))?;
    if !canonical_script.starts_with(&canonical_root) {
        return Err(ExecutorError::ScriptNotFound(path_str));
    }

    validate_arguments(arguments)?;
    let mut env_vars = build_env_vars(arguments)?;

    // Inject W3C trace context into the subprocess environment so that scripts
    // and child processes can propagate the active trace (TRACEPARENT / TRACESTATE).
    let mut trace_context = HashMap::new();
    tumult_otel::propagation::inject_traceparent(
        &opentelemetry::Context::current(),
        &mut trace_context,
    );
    env_vars.extend(
        trace_context
            .into_iter()
            .map(|(key, value)| (key.to_uppercase(), value)),
    );

    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.arg(script_path);
    cmd.envs(&env_vars);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    // Put the shell in its own process group so a timeout can kill the whole
    // group — children the script spawned (e.g. a backgrounded `sleep`) must
    // not survive it. Same pattern as the CLI's process executor.
    #[cfg(unix)]
    cmd.process_group(0);

    cmd.kill_on_drop(true); // Kill child process if future is dropped (timeout)

    let mut child = cmd.spawn().map_err(ExecutorError::ExecutionFailed)?;

    // Drain the pipes concurrently with waiting: a script whose output
    // exceeds the OS pipe buffer would otherwise block on write and be
    // killed as a false timeout.
    let stdout_task = child
        .stdout
        .take()
        .map(|out| tokio::spawn(read_capped(out, "stdout")));
    let stderr_task = child
        .stderr
        .take()
        .map(|err| tokio::spawn(read_capped(err, "stderr")));

    let status = match wait_for_exit(&mut child, timeout, stop).await {
        Waited::Exited(status) => status.map_err(ExecutorError::ExecutionFailed)?,
        Waited::TimedOut(duration) => {
            end_child(&mut child).await;
            log_timed_out(&path_str, duration);
            crate::telemetry::event_script_timed_out(&path_str, duration.as_secs_f64());
            crate::telemetry::record_execution(false);
            return Err(ExecutorError::Timeout(duration.as_secs_f64()));
        }
        Waited::Stopped => {
            end_child(&mut child).await;
            log_stopped(&path_str);
            crate::telemetry::record_execution(false);
            return Err(ExecutorError::Stopped);
        }
    };

    // The reader tasks finish at EOF once the process group is dead.
    let stdout = collect_output(stdout_task, "stdout").await;
    let stderr = collect_output(stderr_task, "stderr").await;

    let exit_status = match status.code() {
        Some(n) => ScriptExitStatus::Code(n),
        None => ScriptExitStatus::Signal,
    };
    let exit_code_for_telemetry = exit_status.code().unwrap_or(-1);
    // Sizes are what the script wrote, not what was kept of it.
    let (stdout_bytes, stderr_bytes) = (stdout.total, stderr.total);
    let result = ScriptResult {
        exit_status,
        stdout_truncated: stdout.truncated,
        stderr_truncated: stderr.truncated,
        stdout: stdout.into_text(),
        stderr: stderr.into_text(),
    };
    log_non_zero_exit(&path_str, &result, stdout_bytes, stderr_bytes);
    crate::telemetry::event_script_completed(exit_code_for_telemetry, stdout_bytes, stderr_bytes);
    crate::telemetry::record_execution(result.succeeded());
    Ok(result)
}

/// How the wait for a script ended.
#[cfg(not(windows))]
enum Waited {
    Exited(std::io::Result<std::process::ExitStatus>),
    TimedOut(Duration),
    Stopped,
}

/// Wait until the script exits, `timeout` passes or `stop` completes,
/// whichever comes first.
#[cfg(not(windows))]
async fn wait_for_exit(
    child: &mut tokio::process::Child,
    timeout: Option<Duration>,
    stop: impl std::future::Future<Output = ()>,
) -> Waited {
    let timed_out = async {
        match timeout {
            Some(duration) => {
                tokio::time::sleep(duration).await;
                duration
            }
            None => std::future::pending().await,
        }
    };
    tokio::select! {
        status = child.wait() => Waited::Exited(status),
        duration = timed_out => Waited::TimedOut(duration),
        () = stop => Waited::Stopped,
    }
}

/// Windows counterpart of the POSIX [`execute_script_until`]: script
/// plugins run through `/bin/sh`, which does not exist on Windows, so
/// dispatch fails fast with a clear error rather than an opaque spawn
/// failure.
///
/// # Errors
///
/// Always returns [`ExecutorError::UnsupportedPlatform`].
#[cfg(windows)]
pub async fn execute_script_until<S: std::hash::BuildHasher>(
    script_path: &Path,
    plugin_root: &Path,
    arguments: &HashMap<String, String, S>,
    timeout: Option<Duration>,
    stop: impl std::future::Future<Output = ()>,
) -> Result<ScriptResult, ExecutorError> {
    let _ = (script_path, plugin_root, arguments, timeout);
    drop(stop);
    Err(ExecutorError::UnsupportedPlatform)
}

/// Most bytes kept from one output stream of a script. A script is
/// third-party code: without a cap, one that writes without end would grow
/// the caller's memory until the process is killed.
#[cfg(not(windows))]
const OUTPUT_CAP: usize = 8 * 1024 * 1024;

/// Size of one read from a script's output pipe.
#[cfg(not(windows))]
const READ_CHUNK_BYTES: usize = 8192;

/// One output stream of a script, as captured.
#[cfg(not(windows))]
#[derive(Debug, Default)]
struct Captured {
    /// At most [`OUTPUT_CAP`] bytes, from the start of the stream.
    bytes: Vec<u8>,
    /// How many bytes the stream held, kept or not.
    total: usize,
    /// Whether the stream held more than was kept.
    truncated: bool,
}

#[cfg(not(windows))]
impl Captured {
    /// Keeps as much of `chunk` as still fits under [`OUTPUT_CAP`].
    fn keep(&mut self, chunk: &[u8]) {
        self.total += chunk.len();
        let room = OUTPUT_CAP - self.bytes.len();
        let kept = chunk.len().min(room);
        self.bytes.extend_from_slice(&chunk[..kept]);
        if kept < chunk.len() {
            self.truncated = true;
        }
    }

    /// The captured bytes as text. Nothing is added to them: output that was
    /// cut ends at the last complete character, so the cut itself never
    /// shows up as a replacement character.
    fn into_text(mut self) -> String {
        if self.truncated {
            let complete = complete_utf8_prefix(&self.bytes);
            self.bytes.truncate(complete);
        }
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

/// Length of `bytes` without a UTF-8 character cut short at its very end.
///
/// Only the last three bytes are looked at, which is as far back as the lead
/// byte of an unfinished character can be; invalid bytes anywhere else are
/// the script's own and are left for the lossy conversion.
#[cfg(not(windows))]
fn complete_utf8_prefix(bytes: &[u8]) -> usize {
    let tail_start = bytes.len().saturating_sub(3);
    for index in (tail_start..bytes.len()).rev() {
        let byte = bytes[index];
        if is_utf8_continuation(byte) {
            continue;
        }
        let ends_inside = index + utf8_width(byte) > bytes.len();
        return if ends_inside { index } else { bytes.len() };
    }
    bytes.len()
}

#[cfg(not(windows))]
fn is_utf8_continuation(byte: u8) -> bool {
    byte & 0b1100_0000 == 0b1000_0000
}

/// How many bytes the character led by `lead` takes; 1 for a byte that leads
/// nothing.
#[cfg(not(windows))]
fn utf8_width(lead: u8) -> usize {
    match lead {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

/// Reads `reader` to EOF, keeping the first [`OUTPUT_CAP`] bytes.
///
/// The stream is drained past the cap so the script never blocks on a full
/// pipe. A read error ends the capture with what arrived so far and is
/// logged; `stream` names the pipe (`stdout` or `stderr`) in log events.
#[cfg(not(windows))]
async fn read_capped<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    stream: &'static str,
) -> Captured {
    use tokio::io::AsyncReadExt as _;
    let mut captured = Captured::default();
    let mut chunk = vec![0_u8; READ_CHUNK_BYTES];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(read) => captured.keep(&chunk[..read]),
            Err(error) => {
                log_read_failed(stream, captured.bytes.len(), &error);
                break;
            }
        }
    }
    if captured.truncated {
        log_truncated(stream, captured.total);
    }
    captured
}

/// Collects a reader task's output. A reader that panicked or was cancelled
/// yields empty output and is logged.
#[cfg(not(windows))]
async fn collect_output(
    task: Option<tokio::task::JoinHandle<Captured>>,
    stream: &'static str,
) -> Captured {
    let Some(handle) = task else {
        return Captured::default();
    };
    match handle.await {
        Ok(captured) => captured,
        Err(error) => {
            log_reader_failed(stream, &error);
            Captured::default()
        }
    }
}

#[cfg(not(windows))]
fn log_read_failed(stream: &'static str, bytes_read: usize, error: &std::io::Error) {
    tracing::warn!(
        name: "plugin.script.output.read_failed",
        {
            process.stream = stream,
            bytes_read,
            error.type = "io",
            error.message = %error,
        },
        "script output read failed, captured output is incomplete"
    );
}

#[cfg(not(windows))]
fn log_truncated(stream: &'static str, bytes_total: usize) {
    tracing::warn!(
        name: "plugin.script.output.truncated",
        { process.stream = stream, max_bytes = OUTPUT_CAP, bytes_total },
        "script output exceeded the capture limit and was truncated"
    );
}

#[cfg(not(windows))]
fn log_reader_failed(stream: &'static str, error: &tokio::task::JoinError) {
    tracing::warn!(
        name: "plugin.script.output.reader_failed",
        {
            process.stream = stream,
            error.type = "join",
            error.message = %error,
        },
        "script output reader did not finish, captured output is empty"
    );
}

#[cfg(not(windows))]
fn log_timed_out(script_path: &str, timeout: Duration) {
    tracing::warn!(
        name: "plugin.script.timed_out",
        { file.path = script_path, timeout_s = timeout.as_secs_f64() },
        "script timed out and was killed"
    );
}

/// Notes a script that was ended because its run was stopped. Someone asked
/// for that, so it is not a warning.
#[cfg(not(windows))]
fn log_stopped(script_path: &str) {
    tracing::info!(
        name: "plugin.script.stopped",
        { file.path = script_path },
        "script was stopped before it finished and was killed"
    );
}

/// Notes a script that did not exit with code 0.
///
/// That is an observation, not a failure of ours: a probe reports a broken
/// steady state this way, and the caller decides what it means. Hence debug.
/// The output is the caller's to handle and is never logged: only its sizes
/// are, counted as written by the script rather than as kept.
#[cfg(not(windows))]
fn log_non_zero_exit(
    script_path: &str,
    result: &ScriptResult,
    stdout_bytes: usize,
    stderr_bytes: usize,
) {
    if result.succeeded() {
        return;
    }
    tracing::debug!(
        name: "plugin.script.exited",
        {
            file.path = script_path,
            process.exit.code = result.exit_status.code(),
            stdout_bytes,
            stderr_bytes,
            stdout_truncated = result.stdout_truncated,
            stderr_truncated = result.stderr_truncated,
        },
        "script exited with a non-zero status"
    );
}

/// Kill the whole process group of a child that timed out or was stopped, so
/// children the script spawned don't survive the shell; fall back to killing
/// only the direct child where there are no process groups. Always reaps the
/// child.
#[cfg(unix)]
async fn end_child(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        kill_process_group(pid);
    }
    let _ = child.wait().await;
}

/// Non-Unix counterpart of [`end_child`].
#[cfg(not(any(unix, windows)))]
async fn end_child(child: &mut tokio::process::Child) {
    let _ = child.kill().await;
}

/// Send `SIGKILL` to the process group whose id is `pid` (the shell is
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_script(dir: &Path, name: &str, content: &str) -> std::path::PathBuf {
        let script_path = dir.join(name);
        tumult_test_utils::write_executable(&script_path, content);
        script_path
    }

    /// [`execute_script_until`] with a stop that never comes.
    async fn run_to_end(
        script_path: &Path,
        plugin_root: &Path,
        arguments: &HashMap<String, String>,
        timeout: Option<Duration>,
    ) -> Result<ScriptResult, ExecutorError> {
        let never = std::future::pending();
        execute_script_until(script_path, plugin_root, arguments, timeout, never).await
    }

    // ── build_env_vars ─────────────────────────────────────────

    #[test]
    fn env_vars_are_uppercased_with_tumult_prefix() {
        let args = HashMap::from([
            ("broker_id".into(), "2".into()),
            ("cluster".into(), "prod".into()),
        ]);
        let env = build_env_vars(&args).unwrap();
        assert_eq!(env.get("TUMULT_BROKER_ID").unwrap(), "2");
        assert_eq!(env.get("TUMULT_CLUSTER").unwrap(), "prod");
        assert_eq!(env.len(), 2);
    }

    #[test]
    fn env_vars_empty_input_returns_empty() {
        let args = HashMap::new();
        let env = build_env_vars(&args).unwrap();
        assert!(env.is_empty());
    }

    #[test]
    fn env_vars_reject_invalid_shell_identifiers() {
        for bad in ["foo-bar", "foo.bar", "1st", "foo bar", "föö"] {
            let args = HashMap::from([(bad.to_string(), "v".to_string())]);
            let result = build_env_vars(&args);
            assert!(
                matches!(result, Err(ExecutorError::InvalidArgumentName(_))),
                "{bad:?} should be rejected, got: {result:?}"
            );
        }
    }

    #[test]
    fn env_vars_reject_case_insensitive_collisions() {
        let args = HashMap::from([
            ("foo".to_string(), "1".to_string()),
            ("FOO".to_string(), "2".to_string()),
        ]);
        let result = build_env_vars(&args);
        match result {
            Err(ExecutorError::ConflictingArgumentNames {
                first,
                second,
                uppercased,
            }) => {
                // Which key is "first" depends on iteration order; both are named.
                assert_eq!(uppercased, "FOO");
                assert!(
                    (first == "foo" && second == "FOO") || (first == "FOO" && second == "foo"),
                    "first={first} second={second}"
                );
            }
            other => panic!("expected ConflictingArgumentNames, got: {other:?}"),
        }
    }

    #[test]
    fn env_vars_accept_underscore_and_digit_suffixes() {
        let args = HashMap::from([
            ("_private".to_string(), "1".to_string()),
            ("port_2".to_string(), "2".to_string()),
        ]);
        let env = build_env_vars(&args).unwrap();
        assert_eq!(env.get("TUMULT__PRIVATE").unwrap(), "1");
        assert_eq!(env.get("TUMULT_PORT_2").unwrap(), "2");
    }

    // ── execute_script ─────────────────────────────────────────

    #[tokio::test]
    async fn execute_script_captures_stdout() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "test.sh", "#!/bin/bash\necho hello");
        let result = run_to_end(&script, dir.path(), &HashMap::new(), None)
            .await
            .unwrap();
        assert_eq!(result.exit_status, ScriptExitStatus::Code(0));
        assert_eq!(result.stdout.trim(), "hello");
        assert!(result.succeeded());
    }

    #[tokio::test]
    async fn execute_script_captures_stderr() {
        let dir = TempDir::new().unwrap();
        let script =
            create_test_script(dir.path(), "test.sh", "#!/bin/bash\necho error >&2\nexit 1");
        let result = run_to_end(&script, dir.path(), &HashMap::new(), None)
            .await
            .unwrap();
        assert_eq!(result.exit_status, ScriptExitStatus::Code(1));
        assert_eq!(result.stderr.trim(), "error");
        assert!(!result.succeeded());
    }

    #[tokio::test]
    async fn execute_script_passes_tumult_env_vars() {
        let dir = TempDir::new().unwrap();
        let script =
            create_test_script(dir.path(), "test.sh", "#!/bin/bash\necho $TUMULT_BROKER_ID");
        let args = HashMap::from([("broker_id".into(), "42".into())]);
        let result = run_to_end(&script, dir.path(), &args, None).await.unwrap();
        assert_eq!(result.stdout.trim(), "42");
    }

    #[tokio::test]
    async fn execute_script_not_found_returns_error() {
        let dir = TempDir::new().unwrap();
        let result = run_to_end(
            Path::new("/nonexistent/script.sh"),
            dir.path(),
            &HashMap::new(),
            None,
        )
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ExecutorError::ScriptNotFound(_)));
    }

    #[tokio::test]
    async fn execute_script_timeout_returns_error() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "test.sh", "#!/bin/bash\nsleep 10");
        let result = run_to_end(
            &script,
            dir.path(),
            &HashMap::new(),
            Some(Duration::from_millis(100)),
        )
        .await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ExecutorError::Timeout(_)));
    }

    #[tokio::test]
    async fn execute_script_timeout_kills_process_group() {
        // A grandchild that outlives the shell must be killed with the group:
        // the script backgrounds a subshell that writes a marker file after a
        // short delay, then sleeps forever. The timeout fires first; if only
        // the shell were killed, the marker would still appear.
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("grandchild-survived");
        let script = create_test_script(
            dir.path(),
            "test.sh",
            &format!(
                "#!/bin/bash\n( sleep 0.4; touch '{}' ) &\nsleep 10\n",
                marker.display()
            ),
        );
        let result = run_to_end(
            &script,
            dir.path(),
            &HashMap::new(),
            Some(Duration::from_millis(100)),
        )
        .await;
        assert!(matches!(result, Err(ExecutorError::Timeout(_))));

        // sleep: there is no event for a file that must stay absent; wait
        // past the time the grandchild would have written it.
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(
            !marker.exists(),
            "grandchild survived the timeout — process group was not killed"
        );
    }

    #[test]
    fn script_result_succeeded_checks_exit_code() {
        let exited = |exit_status| ScriptResult {
            exit_status,
            stdout: String::new(),
            stderr: String::new(),
            stdout_truncated: false,
            stderr_truncated: false,
        };
        assert!(exited(ScriptExitStatus::Code(0)).succeeded());
        assert!(!exited(ScriptExitStatus::Code(1)).succeeded());
        assert!(!exited(ScriptExitStatus::Signal).succeeded());
    }

    #[tokio::test]
    async fn execute_script_hands_the_script_span_to_the_child_as_traceparent() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(
            dir.path(),
            "test.sh",
            "#!/bin/bash\necho \"traceparent=${TRACEPARENT}\"",
        );

        let (result, spans) = tumult_test_utils::capture_spans_async(run_to_end(
            &script,
            dir.path(),
            &HashMap::new(),
            None,
        ))
        .await;

        let span = spans
            .iter()
            .find(|span| span.name == "script.execute")
            .expect("script.execute span is recorded");
        let context = &span.span_context;
        assert_eq!(
            result.unwrap().stdout.trim(),
            format!(
                "traceparent=00-{}-{}-01",
                context.trace_id(),
                context.span_id()
            )
        );
    }

    #[test]
    fn validate_arguments_rejects_empty_key() {
        let args = HashMap::from([(String::new(), "value".into())]);
        let result = validate_arguments(&args);
        assert!(
            matches!(result, Err(ExecutorError::NullByteInArgument(_))),
            "empty key should be rejected"
        );
    }

    #[tokio::test]
    async fn execute_script_rejects_path_traversal() {
        // A script path that escapes the plugin root after canonicalization
        // must be rejected with ScriptNotFound — even if the target file exists.
        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        let escaped_script =
            create_test_script(outside.path(), "evil.sh", "#!/bin/bash\necho pwned");

        let result = run_to_end(&escaped_script, root.path(), &HashMap::new(), None).await;
        assert!(
            matches!(result, Err(ExecutorError::ScriptNotFound(_))),
            "expected ScriptNotFound when script is outside plugin root, got: {result:?}"
        );
    }

    #[test]
    fn validate_arguments_rejects_null_bytes() {
        let in_value = HashMap::from([("key".to_string(), "va\0lue".to_string())]);
        assert!(
            matches!(
                validate_arguments(&in_value),
                Err(ExecutorError::NullByteInArgument(k)) if k == "key"
            ),
            "null byte in value should be rejected naming its key"
        );

        let in_key = HashMap::from([("ke\0y".to_string(), "value".to_string())]);
        assert!(
            matches!(
                validate_arguments(&in_key),
                Err(ExecutorError::NullByteInArgument(_))
            ),
            "null byte in key should be rejected"
        );
    }

    #[tokio::test]
    async fn execute_script_rejects_invalid_argument_names() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "test.sh", "#!/bin/bash\necho hi");
        let args = HashMap::from([("foo-bar".to_string(), "v".to_string())]);
        let result = run_to_end(&script, dir.path(), &args, None).await;
        assert!(
            matches!(result, Err(ExecutorError::InvalidArgumentName(_))),
            "expected InvalidArgumentName, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn execute_script_rejects_conflicting_argument_names() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "test.sh", "#!/bin/bash\necho hi");
        let args = HashMap::from([
            ("foo".to_string(), "1".to_string()),
            ("FOO".to_string(), "2".to_string()),
        ]);
        let result = run_to_end(&script, dir.path(), &args, None).await;
        assert!(
            matches!(result, Err(ExecutorError::ConflictingArgumentNames { .. })),
            "expected ConflictingArgumentNames, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn execute_script_rejects_null_byte_arguments() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "test.sh", "#!/bin/bash\necho hi");
        let args = HashMap::from([("key".to_string(), "va\0lue".to_string())]);
        let result = run_to_end(&script, dir.path(), &args, None).await;
        assert!(
            matches!(result, Err(ExecutorError::NullByteInArgument(_))),
            "expected NullByteInArgument, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn execute_script_reports_signal_termination() {
        // A script that kills itself has no numeric exit code: the executor
        // must report Signal rather than a bogus code.
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "test.sh", "#!/bin/bash\nkill -9 $$");
        let result = run_to_end(&script, dir.path(), &HashMap::new(), None)
            .await
            .unwrap();
        assert_eq!(result.exit_status, ScriptExitStatus::Signal);
        assert_eq!(result.exit_status.code(), None);
        assert!(!result.succeeded());
    }

    // ── output capture and log events ──────────────────────────

    /// Yields some bytes, then fails.
    struct FailingReader {
        served: bool,
    }

    impl tokio::io::AsyncRead for FailingReader {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.served {
                return std::task::Poll::Ready(Err(std::io::Error::other("pipe torn down")));
            }
            self.served = true;
            buf.put_slice(b"partial");
            std::task::Poll::Ready(Ok(()))
        }
    }

    fn capture(level: tracing::Level) -> tumult_otel::logging::testing::LogCapture {
        tumult_otel::logging::testing::LogCapture::start(level)
    }

    #[tokio::test]
    async fn a_failed_pipe_read_keeps_what_arrived_and_is_logged() {
        let logs = capture(tracing::Level::DEBUG);
        let captured = read_capped(FailingReader { served: false }, "stderr").await;
        let logs = logs.snapshot();

        assert_eq!(captured.bytes, b"partial");
        let event = &logs.named("plugin.script.output.read_failed")[0];
        assert_eq!(event["level"], "WARN");
        assert_eq!(event["process.stream"], "stderr");
        assert_eq!(event["bytes_read"], 7);
        assert_eq!(event["error.type"], "io");
        assert_eq!(event["error.message"], "pipe torn down");
    }

    #[tokio::test]
    async fn output_beyond_the_cap_is_dropped_flagged_and_logged() {
        use tokio::io::AsyncReadExt as _;

        let source = tokio::io::repeat(b'x').take(OUTPUT_CAP as u64 + 4096);
        let logs = capture(tracing::Level::DEBUG);
        let captured = read_capped(source, "stdout").await;
        let logs = logs.snapshot();

        assert_eq!(captured.bytes.len(), OUTPUT_CAP);
        assert_eq!(captured.total, OUTPUT_CAP + 4096);
        assert!(captured.truncated);
        let event = &logs.named("plugin.script.output.truncated")[0];
        assert_eq!(event["level"], "WARN");
        assert_eq!(event["process.stream"], "stdout");
        assert_eq!(event["max_bytes"], OUTPUT_CAP);
        assert_eq!(event["bytes_total"], OUTPUT_CAP + 4096);
        // Nothing is added to what the script wrote.
        let text = captured.into_text();
        assert_eq!(text.len(), OUTPUT_CAP);
        assert!(text.bytes().all(|byte| byte == b'x'));
    }

    #[tokio::test]
    async fn output_is_cut_between_characters_not_inside_one() {
        // One ASCII byte, then two-byte characters: the cap, an even number
        // of bytes in, falls inside a character.
        let mut source = String::from("x");
        source.push_str(&"\u{e9}".repeat(OUTPUT_CAP / 2 + 8));
        let _logs = capture(tracing::Level::DEBUG);

        let captured = read_capped(source.as_bytes(), "stdout").await;

        assert!(captured.truncated);
        assert_eq!(captured.total, source.len());
        let text = captured.into_text();
        assert_eq!(
            text.len(),
            OUTPUT_CAP - 1,
            "the split character is left out"
        );
        assert!(source.starts_with(&text), "only what the script wrote");
    }

    #[test]
    fn a_cut_keeps_complete_characters_of_every_width() {
        let cut = |bytes: &[u8]| {
            let captured = Captured {
                bytes: bytes.to_vec(),
                total: bytes.len() + 1,
                truncated: true,
            };
            captured.into_text()
        };
        assert_eq!(cut(b"ab"), "ab");
        assert_eq!(cut("a\u{e9}".as_bytes()), "a\u{e9}");
        assert_eq!(cut(&"a\u{e9}".as_bytes()[..2]), "a");
        assert_eq!(cut("a\u{20ac}".as_bytes()), "a\u{20ac}");
        assert_eq!(cut(&"a\u{20ac}".as_bytes()[..3]), "a");
        assert_eq!(cut("a\u{1f600}".as_bytes()), "a\u{1f600}");
        assert_eq!(cut(&"a\u{1f600}".as_bytes()[..4]), "a");
        assert_eq!(cut(b""), "");
    }

    #[tokio::test]
    async fn a_script_with_too_much_output_says_so_beside_the_output() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(
            dir.path(),
            "loud.sh",
            "#!/bin/sh\nprintf '{\"rows\":\"'\nhead -c 9000000 /dev/zero | tr '\\0' 'x'\necho quiet >&2\nexit 2\n",
        );
        let logs = capture(tracing::Level::DEBUG);
        let result = run_to_end(&script, dir.path(), &HashMap::new(), None)
            .await
            .unwrap();
        let logs = logs.snapshot();

        assert!(result.stdout_truncated);
        assert!(!result.stderr_truncated);
        assert_eq!(result.stdout.len(), OUTPUT_CAP);
        assert!(result.stdout.starts_with("{\"rows\":\"xxx"));
        assert!(result.stdout.ends_with("xxx"), "no note inside the output");
        assert_eq!(result.stderr, "quiet\n");
        let event = &logs.named("plugin.script.exited")[0];
        assert_eq!(event["stdout_bytes"], 9_000_009);
        assert_eq!(event["stderr_bytes"], 6);
        assert_eq!(event["stdout_truncated"], true);
        assert_eq!(event["stderr_truncated"], false);
    }

    #[tokio::test]
    async fn output_within_the_cap_is_returned_unchanged_and_silent() {
        let logs = capture(tracing::Level::DEBUG);
        let captured = read_capped(&b"hello\n"[..], "stdout").await;
        let logs = logs.snapshot();

        assert!(!captured.truncated);
        assert_eq!(captured.into_text(), "hello\n");
        assert!(logs.events.is_empty(), "{}", logs.raw);
    }

    #[tokio::test]
    async fn a_reader_task_that_dies_is_logged_and_yields_empty_output() {
        let dead = tokio::spawn(std::future::pending::<Captured>());
        dead.abort();
        let logs = capture(tracing::Level::DEBUG);
        let captured = collect_output(Some(dead), "stdout").await;
        let logs = logs.snapshot();

        assert!(captured.bytes.is_empty());
        let event = &logs.named("plugin.script.output.reader_failed")[0];
        assert_eq!(event["level"], "WARN");
        assert_eq!(event["process.stream"], "stdout");
        assert!(event["error.message"].is_string(), "{event}");
    }

    #[tokio::test]
    async fn a_non_zero_exit_is_a_debug_observation_and_a_zero_exit_is_silent() {
        let dir = TempDir::new().unwrap();
        let failing = create_test_script(
            dir.path(),
            "fail.sh",
            "#!/bin/sh\necho out\necho problem >&2\nexit 3\n",
        );
        let passing = create_test_script(dir.path(), "ok.sh", "#!/bin/sh\necho out\nexit 0\n");
        let logs = capture(tracing::Level::DEBUG);

        let result = run_to_end(&failing, dir.path(), &HashMap::new(), None)
            .await
            .unwrap();
        let after_the_failing_script = logs.snapshot();
        run_to_end(&passing, dir.path(), &HashMap::new(), None)
            .await
            .unwrap();
        let after_both = logs.snapshot();

        assert_eq!(result.exit_status, ScriptExitStatus::Code(3));
        let exits = after_the_failing_script.named("plugin.script.exited");
        assert_eq!(exits.len(), 1, "{}", after_the_failing_script.raw);
        let event = exits[0];
        assert_eq!(event["level"], "DEBUG");
        assert_eq!(event["file.path"], failing.to_str().unwrap());
        assert_eq!(event["process.exit.code"], 3);
        assert_eq!(event["stdout_bytes"], 4);
        assert_eq!(event["stderr_bytes"], 8);
        assert_eq!(event["stdout_truncated"], false);
        assert!(
            !after_both.raw.contains("problem"),
            "output is not logged: {}",
            after_both.raw
        );
        // The passing script added no exit event, and neither raised a warning.
        assert_eq!(after_both.named("plugin.script.exited").len(), 1);
        let warnings = after_both
            .events
            .iter()
            .filter(|event| event["level"] == "WARN" || event["level"] == "ERROR")
            .count();
        assert_eq!(warnings, 0, "{}", after_both.raw);
    }

    // ── a stop from outside ────────────────────────────────────

    /// A stop that comes after `delay`.
    async fn stop_after(delay: Duration) {
        // sleep: this is the stop itself, a signal at a set time.
        tokio::time::sleep(delay).await;
    }

    #[tokio::test]
    async fn a_stopped_script_ends_at_once_with_an_error_that_says_so() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "slow.sh", "#!/bin/sh\nsleep 30\n");
        let started = std::time::Instant::now();

        let result = execute_script_until(
            &script,
            dir.path(),
            &HashMap::new(),
            Some(Duration::from_secs(60)),
            stop_after(Duration::from_millis(100)),
        )
        .await;

        let error = result.expect_err("a stopped script has no result");
        assert!(matches!(error, ExecutorError::Stopped), "{error}");
        assert_eq!(error.to_string(), "script stopped before it finished");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the stop waited for the script: {:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_stopped_script_takes_its_children_with_it() {
        // The script starts a child that would leave a marker after the
        // stop. If only the shell were killed, the marker would appear.
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("child-survived");
        let script = create_test_script(
            dir.path(),
            "test.sh",
            &format!(
                "#!/bin/sh\n( sleep 0.4; touch \"{}\" ) &\nsleep 30\n",
                marker.display()
            ),
        );

        let result = execute_script_until(
            &script,
            dir.path(),
            &HashMap::new(),
            None,
            stop_after(Duration::from_millis(100)),
        )
        .await;
        assert!(matches!(result, Err(ExecutorError::Stopped)));

        // sleep: there is no event for a file that must stay absent; wait
        // past the time the child would have written it.
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(!marker.exists(), "a child of the script outlived the stop");
    }

    #[tokio::test]
    async fn a_script_that_ends_before_the_stop_keeps_its_result() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "quick.sh", "#!/bin/sh\necho done\n");

        let result = execute_script_until(
            &script,
            dir.path(),
            &HashMap::new(),
            None,
            stop_after(Duration::from_secs(60)),
        )
        .await
        .expect("the script ran to its end");

        assert!(result.succeeded());
        assert_eq!(result.stdout.trim(), "done");
    }

    #[tokio::test]
    async fn a_timeout_that_comes_before_the_stop_is_still_a_timeout() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "slow.sh", "#!/bin/sh\nsleep 30\n");

        let result = execute_script_until(
            &script,
            dir.path(),
            &HashMap::new(),
            Some(Duration::from_millis(100)),
            stop_after(Duration::from_secs(60)),
        )
        .await;

        assert!(matches!(result, Err(ExecutorError::Timeout(_))));
    }

    #[tokio::test]
    async fn a_stopped_script_is_a_named_event_and_not_a_warning() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "slow.sh", "#!/bin/sh\nsleep 30\n");
        let logs = capture(tracing::Level::INFO);
        let result = execute_script_until(
            &script,
            dir.path(),
            &HashMap::new(),
            None,
            stop_after(Duration::from_millis(100)),
        )
        .await;
        let logs = logs.snapshot();

        assert!(matches!(result, Err(ExecutorError::Stopped)));
        let event = &logs.named("plugin.script.stopped")[0];
        assert_eq!(event["level"], "INFO");
        assert_eq!(event["file.path"], script.to_str().unwrap());
    }

    #[tokio::test]
    async fn a_script_timeout_is_a_named_warning() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "slow.sh", "#!/bin/sh\nsleep 10\n");
        let logs = capture(tracing::Level::INFO);
        let result = run_to_end(
            &script,
            dir.path(),
            &HashMap::new(),
            Some(Duration::from_millis(100)),
        )
        .await;
        let logs = logs.snapshot();

        assert!(matches!(result, Err(ExecutorError::Timeout(_))));
        let event = &logs.named("plugin.script.timed_out")[0];
        assert_eq!(event["level"], "WARN");
        assert_eq!(event["file.path"], script.to_str().unwrap());
        assert_eq!(event["timeout_s"], 0.1);
    }
}
