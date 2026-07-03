//! Experiment lifecycle tools: validate, run, and scaffold experiments.

use std::path::Path;

use crate::error::ToolError;

/// Validate an experiment file. Returns a summary string on success.
///
/// # Errors
///
/// Returns a [`ToolError`] if the file cannot be read, parsed, or fails
/// validation.
pub fn validate_experiment(experiment_path: &str) -> Result<String, ToolError> {
    use tumult_core::engine::{parse_experiment, validate_experiment};

    let content = std::fs::read_to_string(Path::new(experiment_path))?;
    let experiment = parse_experiment(&content).map_err(|e| ToolError::Parse(e.to_string()))?;
    validate_experiment(&experiment).map_err(|e| ToolError::Validation(e.to_string()))?;

    Ok(format!(
        "Valid: '{}' — {} method steps, {} rollbacks",
        experiment.title,
        experiment.method.len(),
        experiment.rollbacks.len()
    ))
}

/// Run an experiment and return the journal as TOON.
///
/// `parent_context` is an optional `OTel` context to link the root
/// `resilience.experiment` span to an upstream caller (e.g. an MCP tool span).
///
/// # Errors
///
/// Returns a [`ToolError`] if the file cannot be read, parsed, validated,
/// executed, or encoded.
pub fn run_experiment(
    experiment_path: &str,
    rollback_strategy: &str,
    parent_context: Option<opentelemetry::Context>,
) -> Result<String, ToolError> {
    use std::sync::Arc;
    use tumult_core::controls::ControlRegistry;
    use tumult_core::engine::{parse_experiment, validate_experiment};
    use tumult_core::execution::RollbackStrategy;
    use tumult_core::journal::encode_journal;
    use tumult_core::runner::{run_experiment as run, ActivityExecutor, RunConfig};

    let content = std::fs::read_to_string(Path::new(experiment_path))?;
    let experiment = parse_experiment(&content).map_err(|e| ToolError::Parse(e.to_string()))?;
    validate_experiment(&experiment).map_err(|e| ToolError::Validation(e.to_string()))?;

    let strategy = match rollback_strategy {
        "always" => RollbackStrategy::Always,
        "never" => RollbackStrategy::Never,
        _ => RollbackStrategy::OnDeviation,
    };

    let executor: Arc<dyn ActivityExecutor> = Arc::new(crate::handler::ProcessExecutor);
    let controls = Arc::new(ControlRegistry::new());
    let config = RunConfig {
        rollback_strategy: strategy,
        cancellation_token: None,
        parent_context,
        load_executor: None,
    };

    let journal = run(&experiment, &executor, &controls, &config)
        .map_err(|e| ToolError::Execution(e.to_string()))?;
    encode_journal(&journal).map_err(|e| ToolError::Execution(e.to_string()))
}

/// Create an experiment file from a template.
///
/// # Errors
///
/// Returns [`ToolError::AlreadyExists`] if the file already exists, or
/// [`ToolError::Io`] if the file cannot be written.
pub fn create_experiment(output_path: &str, plugin: Option<&str>) -> Result<String, ToolError> {
    let path = Path::new(output_path);
    if path.exists() {
        return Err(ToolError::AlreadyExists(format!(
            "{output_path} already exists"
        )));
    }

    let plugin_name = plugin.unwrap_or("tumult-example");
    let template = format!(
        r#"title: My chaos experiment
description: Describe what this experiment validates

tags[2]: resilience, testing

steady_state_hypothesis:
  title: System is reachable
  probes[1]:
    - name: system-check
      activity_type: probe
      provider:
        type: process
        path: uname
        arguments[1]: "-a"
        timeout_s: 5.0
      tolerance:
        type: regex
        pattern: "."

method[1]:
  - name: inject-fault
    activity_type: action
    provider:
      type: process
      path: echo
      arguments[1]: "fault injected via {plugin_name}"
      timeout_s: 30.0
"#
    );

    std::fs::write(path, &template)?;
    Ok(format!("Created {output_path}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::write_valid_experiment;
    use tempfile::TempDir;

    // ── validate_experiment ───────────────────────────────────

    #[test]
    fn validate_valid_experiment_succeeds() {
        let dir = TempDir::new().unwrap();
        let path = write_valid_experiment(dir.path());
        let result = validate_experiment(&path);
        assert!(result.is_ok());
        assert!(result.unwrap().contains("MCP test experiment"));
    }

    #[test]
    fn validate_nonexistent_file_returns_error() {
        let result = validate_experiment("/nonexistent/file.toon");
        assert!(result.is_err());
    }

    #[test]
    fn validate_invalid_toon_returns_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("bad.toon");
        std::fs::write(&path, "not valid toon {{{").unwrap();
        let result = validate_experiment(path.to_str().unwrap());
        assert!(result.is_err());
    }

    // ── run_experiment ────────────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_valid_experiment_returns_journal() {
        let dir = TempDir::new().unwrap();
        let path = write_valid_experiment(dir.path());
        let result = run_experiment(&path, "on-deviation", None);
        assert!(result.is_ok());
        let journal = result.unwrap();
        assert!(journal.contains("MCP test experiment"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_nonexistent_returns_error() {
        let result = run_experiment("/nonexistent.toon", "always", None);
        assert!(result.is_err());
    }

    // ── create_experiment ─────────────────────────────────────

    #[test]
    fn create_experiment_writes_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("new.toon");
        let result = create_experiment(path.to_str().unwrap(), None);
        assert!(result.is_ok());
        assert!(path.exists());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("title:"));
    }

    #[test]
    fn create_experiment_with_plugin() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("kafka.toon");
        let result = create_experiment(path.to_str().unwrap(), Some("tumult-kafka"));
        assert!(result.is_ok());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("tumult-kafka"));
    }

    #[test]
    fn create_experiment_fails_if_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("existing.toon");
        std::fs::write(&path, "existing").unwrap();
        let result = create_experiment(path.to_str().unwrap(), None);
        assert!(result.is_err());
    }
}
