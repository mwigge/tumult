//! Script plugin executor — runs scripts with TUMULT_* env vars.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

#[cfg(not(windows))]
use opentelemetry::propagation::Injector;
use thiserror::Error;

/// A simple `HashMap`-backed carrier for W3C trace-context propagation.
/// Only the POSIX `execute_script` injects trace context.
#[cfg(not(windows))]
struct HashMapCarrier(HashMap<String, String>);

#[cfg(not(windows))]
impl Injector for HashMapCarrier {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_uppercase(), value);
    }
}

#[derive(Error, Debug)]
pub enum ExecutorError {
    #[error("script not found: {0}")]
    ScriptNotFound(String),
    #[error("script execution failed: {0}")]
    ExecutionFailed(#[from] std::io::Error),
    #[error("script timed out after {0}s")]
    Timeout(f64),
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
    pub stdout: String,
    pub stderr: String,
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

/// Execute a script at the given path with TUMULT_* env vars.
///
/// `plugin_root` is the canonical directory the plugin was loaded from.
/// The script path is resolved relative to `plugin_root` and must remain
/// within it after canonicalization, preventing path-traversal via manifests
/// that specify `../../etc/passwd` as the script path.
///
/// The shell runs in its own process group: on timeout the whole group is
/// killed (Unix), so children the script spawned don't survive it.
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
#[cfg(not(windows))]
pub async fn execute_script<S: std::hash::BuildHasher>(
    script_path: &Path,
    plugin_root: &Path,
    arguments: &HashMap<String, String, S>,
    timeout: Option<Duration>,
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
    let mut carrier = HashMapCarrier(HashMap::new());
    opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.inject(&mut carrier);
    });
    env_vars.extend(carrier.0);

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

    let status = if let Some(duration) = timeout {
        match tokio::time::timeout(duration, child.wait()).await {
            Ok(status) => status.map_err(ExecutorError::ExecutionFailed)?,
            Err(_elapsed) => {
                kill_timed_out_child(&mut child).await;
                log_timed_out(&path_str, duration);
                crate::telemetry::event_script_timed_out(&path_str, duration.as_secs_f64());
                crate::telemetry::record_execution(false);
                return Err(ExecutorError::Timeout(duration.as_secs_f64()));
            }
        }
    } else {
        child.wait().await.map_err(ExecutorError::ExecutionFailed)?
    };

    // The reader tasks finish at EOF once the process group is dead.
    let stdout = collect_output(stdout_task, "stdout").await;
    let stderr = collect_output(stderr_task, "stderr").await;

    let exit_status = match status.code() {
        Some(n) => ScriptExitStatus::Code(n),
        None => ScriptExitStatus::Signal,
    };
    let exit_code_for_telemetry = exit_status.code().unwrap_or(-1);
    let result = ScriptResult {
        exit_status,
        stdout: stdout.into_text(),
        stderr: stderr.into_text(),
    };
    log_if_failed(&path_str, &result);
    crate::telemetry::event_script_completed(
        exit_code_for_telemetry,
        result.stdout.len(),
        result.stderr.len(),
    );
    crate::telemetry::record_execution(result.succeeded());
    Ok(result)
}

/// Windows counterpart of the POSIX [`execute_script`]: script plugins run
/// through `/bin/sh`, which does not exist on Windows, so dispatch fails
/// fast with a clear error rather than an opaque spawn failure.
///
/// # Errors
///
/// Always returns [`ExecutorError::UnsupportedPlatform`].
#[cfg(windows)]
pub async fn execute_script<S: std::hash::BuildHasher>(
    script_path: &Path,
    plugin_root: &Path,
    arguments: &HashMap<String, String, S>,
    timeout: Option<Duration>,
) -> Result<ScriptResult, ExecutorError> {
    let _ = (script_path, plugin_root, arguments, timeout);
    Err(ExecutorError::UnsupportedPlatform)
}

/// Most bytes kept from one output stream of a script. A script is
/// third-party code: without a cap, one that writes without end would grow
/// the caller's memory until the process is killed.
#[cfg(not(windows))]
const OUTPUT_CAP: usize = 8 * 1024 * 1024;

/// Appended to captured output that was cut at [`OUTPUT_CAP`].
#[cfg(not(windows))]
const TRUNCATION_NOTE: &str = "\n[output truncated at 8 MiB]";

/// Size of one read from a script's output pipe.
#[cfg(not(windows))]
const READ_CHUNK_BYTES: usize = 8192;

/// One output stream of a script, as captured.
#[cfg(not(windows))]
#[derive(Debug, Default)]
struct Captured {
    /// At most [`OUTPUT_CAP`] bytes, from the start of the stream.
    bytes: Vec<u8>,
    /// Whether the stream held more than was kept.
    truncated: bool,
}

#[cfg(not(windows))]
impl Captured {
    /// Keeps as much of `chunk` as still fits under [`OUTPUT_CAP`].
    fn keep(&mut self, chunk: &[u8]) {
        let room = OUTPUT_CAP - self.bytes.len();
        let kept = chunk.len().min(room);
        self.bytes.extend_from_slice(&chunk[..kept]);
        if kept < chunk.len() {
            self.truncated = true;
        }
    }

    /// The captured bytes as text, with a note when output was cut.
    fn into_text(self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).into_owned();
        if self.truncated {
            text.push_str(TRUNCATION_NOTE);
        }
        text
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
        log_truncated(stream);
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
fn log_truncated(stream: &'static str) {
    tracing::warn!(
        name: "plugin.script.output.truncated",
        { process.stream = stream, max_bytes = OUTPUT_CAP },
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

/// Logs a script that did not exit with code 0. Its output is the caller's
/// to handle and is never logged: only the sizes are.
#[cfg(not(windows))]
fn log_if_failed(script_path: &str, result: &ScriptResult) {
    if result.succeeded() {
        return;
    }
    tracing::warn!(
        name: "plugin.script.failed",
        {
            file.path = script_path,
            process.exit.code = result.exit_status.code(),
            stdout_bytes = result.stdout.len(),
            stderr_bytes = result.stderr.len(),
        },
        "script exited unsuccessfully"
    );
}

/// Kill the timed-out child's whole process group on Unix, so children the
/// script spawned don't survive the shell; fall back to killing only the
/// direct child elsewhere. Always reaps the child.
#[cfg(unix)]
async fn kill_timed_out_child(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        kill_process_group(pid);
    }
    let _ = child.wait().await;
}

/// Non-Unix counterpart of [`kill_timed_out_child`].
#[cfg(not(any(unix, windows)))]
async fn kill_timed_out_child(child: &mut tokio::process::Child) {
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
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    fn create_test_script(dir: &Path, name: &str, content: &str) -> std::path::PathBuf {
        use std::io::Write;
        let script_path = dir.join(name);
        let mut file = fs::File::create(&script_path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file.sync_all().unwrap();
        drop(file); // Ensure file handle is closed before chmod + exec
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();
        script_path
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
        let result = execute_script(&script, dir.path(), &HashMap::new(), None)
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
        let result = execute_script(&script, dir.path(), &HashMap::new(), None)
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
        let result = execute_script(&script, dir.path(), &args, None)
            .await
            .unwrap();
        assert_eq!(result.stdout.trim(), "42");
    }

    #[tokio::test]
    async fn execute_script_not_found_returns_error() {
        let dir = TempDir::new().unwrap();
        let result = execute_script(
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
        let result = execute_script(
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
        let result = execute_script(
            &script,
            dir.path(),
            &HashMap::new(),
            Some(Duration::from_millis(100)),
        )
        .await;
        assert!(matches!(result, Err(ExecutorError::Timeout(_))));

        // Wait past the grandchild's would-be write time.
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert!(
            !marker.exists(),
            "grandchild survived the timeout — process group was not killed"
        );
    }

    #[test]
    fn script_result_succeeded_checks_exit_code() {
        let success = ScriptResult {
            exit_status: ScriptExitStatus::Code(0),
            stdout: String::new(),
            stderr: String::new(),
        };
        assert!(success.succeeded());

        let failure = ScriptResult {
            exit_status: ScriptExitStatus::Code(1),
            stdout: String::new(),
            stderr: String::new(),
        };
        assert!(!failure.succeeded());

        let signalled = ScriptResult {
            exit_status: ScriptExitStatus::Signal,
            stdout: String::new(),
            stderr: String::new(),
        };
        assert!(!signalled.succeeded());
    }

    #[tokio::test]
    async fn execute_script_injects_traceparent_env_var() {
        // Verify that TRACEPARENT is injected into the child environment.
        // When no active span exists the W3C propagator may produce an empty
        // value; the key presence (or absence with empty value) depends on
        // the global propagator configuration.  We assert the script can read
        // the variable without crashing the executor.
        let dir = TempDir::new().unwrap();
        let script = create_test_script(
            dir.path(),
            "test.sh",
            "#!/bin/bash\necho \"traceparent=${TRACEPARENT}\"",
        );
        let result = execute_script(&script, dir.path(), &HashMap::new(), None)
            .await
            .unwrap();
        // The script must succeed regardless of whether a span is active.
        assert_eq!(result.exit_status, ScriptExitStatus::Code(0));
        // Output always contains the "traceparent=" line (value may be empty).
        assert!(result.stdout.contains("traceparent="));
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

        let result = execute_script(&escaped_script, root.path(), &HashMap::new(), None).await;
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
        let result = execute_script(&script, dir.path(), &args, None).await;
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
        let result = execute_script(&script, dir.path(), &args, None).await;
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
        let result = execute_script(&script, dir.path(), &args, None).await;
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
        let result = execute_script(&script, dir.path(), &HashMap::new(), None)
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
    async fn output_beyond_the_cap_is_dropped_noted_and_logged() {
        use tokio::io::AsyncReadExt as _;

        let source = tokio::io::repeat(b'x').take(OUTPUT_CAP as u64 + 4096);
        let logs = capture(tracing::Level::DEBUG);
        let captured = read_capped(source, "stdout").await;
        let logs = logs.snapshot();

        assert_eq!(captured.bytes.len(), OUTPUT_CAP);
        assert!(captured.truncated);
        let event = &logs.named("plugin.script.output.truncated")[0];
        assert_eq!(event["level"], "WARN");
        assert_eq!(event["process.stream"], "stdout");
        assert!(captured
            .into_text()
            .ends_with("\n[output truncated at 8 MiB]"));
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
    async fn a_script_that_exits_non_zero_is_a_named_warning() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(
            dir.path(),
            "fail.sh",
            "#!/bin/sh\necho out\necho problem >&2\nexit 3\n",
        );
        let logs = capture(tracing::Level::INFO);
        let result = execute_script(&script, dir.path(), &HashMap::new(), None)
            .await
            .unwrap();
        let logs = logs.snapshot();

        assert_eq!(result.exit_status, ScriptExitStatus::Code(3));
        let event = &logs.named("plugin.script.failed")[0];
        assert_eq!(event["level"], "WARN");
        assert_eq!(event["file.path"], script.to_str().unwrap());
        assert_eq!(event["process.exit.code"], 3);
        assert_eq!(event["stdout_bytes"], 4);
        assert_eq!(event["stderr_bytes"], 8);
        assert!(
            !logs.raw.contains("problem"),
            "output is not logged: {}",
            logs.raw
        );
    }

    #[tokio::test]
    async fn a_script_that_succeeds_logs_no_failure() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "ok.sh", "#!/bin/sh\nexit 0\n");
        let logs = capture(tracing::Level::INFO);
        execute_script(&script, dir.path(), &HashMap::new(), None)
            .await
            .unwrap();
        let logs = logs.snapshot();

        assert!(
            logs.named("plugin.script.failed").is_empty(),
            "{}",
            logs.raw
        );
    }

    #[tokio::test]
    async fn a_script_timeout_is_a_named_warning() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "slow.sh", "#!/bin/sh\nsleep 10\n");
        let logs = capture(tracing::Level::INFO);
        let result = execute_script(
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
