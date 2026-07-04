//! Experiment lifecycle tools: validate, run, and scaffold experiments.

use std::path::Path;

use crate::error::ToolError;
use crate::tools::StructuredReport;

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

/// Parameters for [`run_experiment`].
pub struct RunExperimentRequest<'a> {
    /// Path to the experiment `.toon` file (already resolved/contained).
    pub experiment_path: &'a str,
    /// One of `on-deviation`, `always`, `never`.
    pub rollback_strategy: &'a str,
    /// Where the journal is written (already resolved/contained).
    pub journal_path: &'a Path,
    /// Analytics store the journal is ingested into (unless `no_ingest`).
    pub store_path: &'a str,
    /// Skip analytics-store ingestion (parity with the CLI's `--no-ingest`).
    pub no_ingest: bool,
    /// Text content format: `json` (default) or `toon`.
    pub format: &'a str,
    /// Optional `OTel` context to link the root `resilience.experiment` span
    /// to an upstream caller (e.g. an MCP tool span).
    pub parent_context: Option<opentelemetry::Context>,
}

/// Parse a rollback strategy string strictly.
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] listing the valid values when the
/// string is not one of `on-deviation`, `always`, `never`.
fn parse_rollback_strategy(
    strategy: &str,
) -> Result<tumult_core::execution::RollbackStrategy, ToolError> {
    use tumult_core::execution::RollbackStrategy;
    match strategy {
        "on-deviation" => Ok(RollbackStrategy::OnDeviation),
        "always" => Ok(RollbackStrategy::Always),
        "never" => Ok(RollbackStrategy::Never),
        other => Err(ToolError::InvalidInput(format!(
            "unknown rollback_strategy '{other}'; valid values: on-deviation, always, never"
        ))),
    }
}

/// Outcome of the post-run ingestion step, reported to the client.
enum IngestionStatus {
    Ingested,
    Duplicate,
    Skipped,
    Failed(String),
}

impl std::fmt::Display for IngestionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ingested => f.write_str("ingested"),
            Self::Duplicate => f.write_str("duplicate"),
            Self::Skipped => f.write_str("skipped"),
            Self::Failed(e) => write!(f, "failed: {e}"),
        }
    }
}

/// Run an experiment, persist the journal, and ingest it into the analytics
/// store (mirroring `tumult run`'s journal write + auto-ingest behavior).
///
/// Returns a [`StructuredReport`]: the structured object always contains
/// `journal` (the full journal as JSON), `journal_path`, and `ingestion`
/// (`ingested` | `duplicate` | `skipped` | `failed: <reason>`). The text
/// content is that object as pretty JSON (`format: "json"`, default) or the
/// journal encoded as TOON (`format: "toon"`); either is capped at 512 KiB.
///
/// Ingestion failures are reported in `ingestion` rather than failing the
/// call, matching the CLI's warning-only behavior.
///
/// # Errors
///
/// Returns a [`ToolError`] if the file cannot be read, parsed, or validated,
/// the rollback strategy or format is invalid, execution fails, or the
/// journal cannot be encoded or written.
pub fn run_experiment(request: RunExperimentRequest<'_>) -> Result<StructuredReport, ToolError> {
    use std::sync::Arc;
    use tumult_core::controls::ControlRegistry;
    use tumult_core::engine::{parse_experiment, validate_experiment};
    use tumult_core::journal::{encode_journal, write_journal};
    use tumult_core::runner::{run_experiment as run, ActivityExecutor, RunConfig};

    if request.format != "json" && request.format != "toon" {
        return Err(ToolError::InvalidInput(format!(
            "unsupported format '{}'; expected json or toon",
            request.format
        )));
    }
    let strategy = parse_rollback_strategy(request.rollback_strategy)?;

    let content = std::fs::read_to_string(Path::new(request.experiment_path))?;
    let experiment = parse_experiment(&content).map_err(|e| ToolError::Parse(e.to_string()))?;
    validate_experiment(&experiment).map_err(|e| ToolError::Validation(e.to_string()))?;

    let executor: Arc<dyn ActivityExecutor> = Arc::new(crate::handler::ProcessExecutor);
    let controls = Arc::new(ControlRegistry::new());
    let config = RunConfig {
        rollback_strategy: strategy,
        cancellation_token: None,
        parent_context: request.parent_context,
        load_executor: None,
        max_concurrent_faults: None,
    };

    let journal = run(&experiment, &executor, &controls, &config)
        .map_err(|e| ToolError::Execution(e.to_string()))?;

    // Persist the journal (CLI parity: `tumult run` always writes it).
    write_journal(&journal, request.journal_path)
        .map_err(|e| ToolError::Execution(format!("failed to write journal: {e}")))?;

    // Auto-ingest into the persistent analytics store (CLI parity:
    // ingest failures are warnings, not run failures).
    let ingestion = if request.no_ingest {
        IngestionStatus::Skipped
    } else {
        match ingest_journal(&journal, &experiment, request.store_path) {
            Ok(true) => IngestionStatus::Ingested,
            Ok(false) => IngestionStatus::Duplicate,
            Err(e) => IngestionStatus::Failed(e.to_string()),
        }
    };

    let mut structured = serde_json::Map::new();
    structured.insert(
        "journal".into(),
        serde_json::to_value(&journal).map_err(|e| ToolError::Execution(e.to_string()))?,
    );
    structured.insert(
        "journal_path".into(),
        serde_json::Value::String(request.journal_path.display().to_string()),
    );
    structured.insert(
        "ingestion".into(),
        serde_json::Value::String(ingestion.to_string()),
    );

    let text = if request.format == "toon" {
        encode_journal(&journal).map_err(|e| ToolError::Execution(e.to_string()))?
    } else {
        serde_json::to_string_pretty(&serde_json::Value::Object(structured.clone()))
            .map_err(|e| ToolError::Execution(e.to_string()))?
    };

    Ok(StructuredReport {
        text: crate::tools::cap_text(text, "full journal persisted at journal_path"),
        structured,
    })
}

/// Ingest a journal into the persistent `DuckDB` analytics store, as the
/// CLI's auto-ingest does. Returns `Ok(false)` for duplicates.
///
/// The experiment definition is passed through so `ChaosGraph` records the full
/// `Fault = plugin::function` + `Service` model for this run.
fn ingest_journal(
    journal: &tumult_core::types::Journal,
    experiment: &tumult_core::types::Experiment,
    store_path: &str,
) -> Result<bool, ToolError> {
    let store = tumult_analytics::AnalyticsStore::open(Path::new(store_path))
        .map_err(|e| ToolError::Store(e.to_string()))?;
    store
        .ingest_journal_with_experiment(journal, Some(experiment))
        .map_err(|e| ToolError::Store(e.to_string()))
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

    /// Build a run request with journal + store inside `dir`.
    fn run_request<'a>(
        experiment_path: &'a str,
        strategy: &'a str,
        journal_path: &'a std::path::Path,
        store_path: &'a str,
        no_ingest: bool,
        format: &'a str,
    ) -> RunExperimentRequest<'a> {
        RunExperimentRequest {
            experiment_path,
            rollback_strategy: strategy,
            journal_path,
            store_path,
            no_ingest,
            format,
            parent_context: None,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_valid_experiment_returns_journal_and_persists() {
        let dir = TempDir::new().unwrap();
        let path = write_valid_experiment(dir.path());
        let journal_path = dir.path().join("journal.toon");
        let store_path = dir.path().join("analytics.duckdb");

        let result = run_experiment(run_request(
            &path,
            "on-deviation",
            &journal_path,
            store_path.to_str().unwrap(),
            false,
            "json",
        ))
        .unwrap();

        assert!(result.text.contains("MCP test experiment"));
        assert_eq!(
            result.structured.get("ingestion").and_then(|v| v.as_str()),
            Some("ingested")
        );
        assert!(journal_path.exists(), "journal must be written to disk");

        // The persisted journal decodes back to the same experiment.
        let journal = tumult_core::journal::read_journal(&journal_path).unwrap();
        assert_eq!(journal.experiment_title, "MCP test experiment");

        // The run was ingested into the analytics store.
        let store = tumult_analytics::AnalyticsStore::open(&store_path).unwrap();
        assert_eq!(store.stats().unwrap().experiment_count, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_with_no_ingest_skips_store() {
        let dir = TempDir::new().unwrap();
        let path = write_valid_experiment(dir.path());
        let journal_path = dir.path().join("journal.toon");
        let store_path = dir.path().join("analytics.duckdb");

        let result = run_experiment(run_request(
            &path,
            "always",
            &journal_path,
            store_path.to_str().unwrap(),
            true,
            "toon",
        ))
        .unwrap();

        assert_eq!(
            result.structured.get("ingestion").and_then(|v| v.as_str()),
            Some("skipped")
        );
        assert!(
            !store_path.exists(),
            "no store must be created when no_ingest is set"
        );
        // TOON format returns the journal itself as text.
        assert!(result.text.contains("experiment_title"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_rejects_unknown_rollback_strategy() {
        let dir = TempDir::new().unwrap();
        let path = write_valid_experiment(dir.path());
        let journal_path = dir.path().join("journal.toon");

        let err = run_experiment(run_request(
            &path,
            "sometimes",
            &journal_path,
            "unused.duckdb",
            true,
            "json",
        ))
        .expect_err("unknown rollback strategy must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("sometimes"), "must name the bad value: {msg}");
        assert!(
            msg.contains("on-deviation") && msg.contains("always") && msg.contains("never"),
            "must list valid values: {msg}"
        );
        assert!(!journal_path.exists(), "no run must have happened");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_rejects_unknown_format() {
        let dir = TempDir::new().unwrap();
        let path = write_valid_experiment(dir.path());
        let journal_path = dir.path().join("journal.toon");

        let err = run_experiment(run_request(
            &path,
            "always",
            &journal_path,
            "unused.duckdb",
            true,
            "yaml",
        ))
        .expect_err("unknown format must be rejected");
        assert!(err.to_string().contains("expected json or toon"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_nonexistent_returns_error() {
        let dir = TempDir::new().unwrap();
        let journal_path = dir.path().join("journal.toon");
        let result = run_experiment(run_request(
            "/nonexistent.toon",
            "always",
            &journal_path,
            "unused.duckdb",
            true,
            "json",
        ));
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
