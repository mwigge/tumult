//! Script plugin executor — runs scripts with TUMULT_* env vars.

use std::collections::HashMap;
use std::path::Path;
use std::process::Output;
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
    #[error("null byte in script argument key or value: {0}")]
    NullByteInArgument(String),
}

/// Result of executing a script plugin action or probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ScriptResult {
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.exit_code == 0
    }
}

/// Validate that no argument keys or values contain null bytes.
///
/// Null bytes in environment variable names or values can cause truncation
/// or injection issues in child processes.
///
/// # Errors
///
/// Returns [`ExecutorError::NullByteInArgument`] if any key or value contains a
/// null byte (`\0`).
pub fn validate_arguments<S: std::hash::BuildHasher>(
    arguments: &HashMap<String, String, S>,
) -> Result<(), ExecutorError> {
    for (k, v) in arguments {
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
/// # Errors
///
/// Returns [`ExecutorError::ScriptNotFound`] if the script path does not exist.
/// Returns [`ExecutorError::NullByteInArgument`] if any argument contains a null byte.
/// Returns [`ExecutorError::ExecutionFailed`] if the process cannot be spawned.
/// Returns [`ExecutorError::Timeout`] if the script does not finish within the given duration.
pub async fn execute_script<S: std::hash::BuildHasher>(
    script_path: &Path,
    arguments: &HashMap<String, String, S>,
    timeout: Option<Duration>,
) -> Result<ScriptResult, ExecutorError> {
    let timeout_f64 = timeout.map(|d| d.as_secs_f64());
    let _span = crate::telemetry::begin_execute(&script_path.display().to_string(), timeout_f64);
    crate::telemetry::event_script_started(&script_path.display().to_string());

    if !script_path.exists() {
        return Err(ExecutorError::ScriptNotFound(
            script_path.display().to_string(),
        ));
    }

    validate_arguments(arguments)?;
    let env_vars = build_env_vars(arguments);

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
            crate::telemetry::event_script_timed_out(
                &script_path.display().to_string(),
                duration.as_secs_f64(),
            );
            crate::telemetry::record_execution(false);
            return Err(ExecutorError::Timeout(duration.as_secs_f64()));
        }
    } else {
        cmd.output().await?
    };

    let result = ScriptResult {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    };
    crate::telemetry::event_script_completed(
        result.exit_code,
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
        let result = execute_script(&script, &HashMap::new(), None)
            .await
            .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "hello");
        assert!(result.succeeded());
    }

    #[tokio::test]
    async fn execute_script_captures_stderr() {
        let dir = TempDir::new().unwrap();
        let script =
            create_test_script(dir.path(), "test.sh", "#!/bin/bash\necho error >&2\nexit 1");
        let result = execute_script(&script, &HashMap::new(), None)
            .await
            .unwrap();
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.stderr.trim(), "error");
        assert!(!result.succeeded());
    }

    #[tokio::test]
    async fn execute_script_passes_tumult_env_vars() {
        let dir = TempDir::new().unwrap();
        let script =
            create_test_script(dir.path(), "test.sh", "#!/bin/bash\necho $TUMULT_BROKER_ID");
        let args = HashMap::from([("broker_id".into(), "42".into())]);
        let result = execute_script(&script, &args, None).await.unwrap();
        assert_eq!(result.stdout.trim(), "42");
    }

    #[tokio::test]
    async fn execute_script_not_found_returns_error() {
        let result =
            execute_script(Path::new("/nonexistent/script.sh"), &HashMap::new(), None).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ExecutorError::ScriptNotFound(_)));
    }

    #[tokio::test]
    async fn execute_script_timeout_returns_error() {
        let dir = TempDir::new().unwrap();
        let script = create_test_script(dir.path(), "test.sh", "#!/bin/bash\nsleep 10");
        let result =
            execute_script(&script, &HashMap::new(), Some(Duration::from_millis(100))).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, ExecutorError::Timeout(_)));
    }

    #[test]
    fn script_result_succeeded_checks_exit_code() {
        let success = ScriptResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        };
        assert!(success.succeeded());

        let failure = ScriptResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: String::new(),
        };
        assert!(!failure.succeeded());
    }
}
