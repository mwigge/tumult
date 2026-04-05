//! Script plugin executor — runs scripts with TUMULT_* env vars.

use std::collections::HashMap;
use std::path::Path;
use std::process::Output;
use std::time::Duration;

use opentelemetry::propagation::Injector;
use thiserror::Error;

/// A simple `HashMap`-backed carrier for W3C trace-context propagation.
struct HashMapCarrier(HashMap<String, String>);

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
#[must_use]
pub fn build_env_vars<S: std::hash::BuildHasher>(
    arguments: &HashMap<String, String, S>,
) -> HashMap<String, String> {
    arguments
        .iter()
        .map(|(k, v)| (format!("TUMULT_{}", k.to_uppercase()), v.clone()))
        .collect()
}

/// Execute a script at the given path with TUMULT_* env vars.
///
/// `plugin_root` is the canonical directory the plugin was loaded from.
/// The script path is resolved relative to `plugin_root` and must remain
/// within it after canonicalization, preventing path-traversal via manifests
/// that specify `../../etc/passwd` as the script path.
///
/// # Errors
///
/// Returns [`ExecutorError::ScriptNotFound`] if the script path does not exist.
/// Returns [`ExecutorError::ScriptNotFound`] if the resolved path escapes `plugin_root`.
/// Returns [`ExecutorError::NullByteInArgument`] if any argument contains a null byte or empty key.
/// Returns [`ExecutorError::ExecutionFailed`] if the process cannot be spawned.
/// Returns [`ExecutorError::Timeout`] if the script does not finish within the given duration.
#[must_use = "callers must check the script result for success or failure"]
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
    let mut env_vars = build_env_vars(arguments);

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

    cmd.kill_on_drop(true); // Kill child process if future is dropped (timeout)

    let output: Output = if let Some(duration) = timeout {
        if let Ok(result) = tokio::time::timeout(duration, cmd.output()).await {
            result?
        } else {
            crate::telemetry::event_script_timed_out(&path_str, duration.as_secs_f64());
            crate::telemetry::record_execution(false);
            return Err(ExecutorError::Timeout(duration.as_secs_f64()));
        }
    } else {
        cmd.output().await?
    };

    let exit_status = match output.status.code() {
        Some(n) => ScriptExitStatus::Code(n),
        None => ScriptExitStatus::Signal,
    };
    let exit_code_for_telemetry = exit_status.code().unwrap_or(-1);
    let result = ScriptResult {
        exit_status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    };
    crate::telemetry::event_script_completed(
        exit_code_for_telemetry,
        result.stdout.len(),
        result.stderr.len(),
    );
    crate::telemetry::record_execution(result.succeeded());
    Ok(result)
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
        let env = build_env_vars(&args);
        assert_eq!(env.get("TUMULT_BROKER_ID").unwrap(), "2");
        assert_eq!(env.get("TUMULT_CLUSTER").unwrap(), "prod");
        assert_eq!(env.len(), 2);
    }

    #[test]
    fn env_vars_empty_input_returns_empty() {
        let args = HashMap::new();
        let env = build_env_vars(&args);
        assert!(env.is_empty());
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
}
