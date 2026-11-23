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
    use tumult_core::engine::{parse_experiment, validate_experiment, validate_placeholders};

    let content = std::fs::read_to_string(Path::new(experiment_path))?;
    let experiment = parse_experiment(&content).map_err(|e| ToolError::Parse(e.to_string()))?;
    validate_experiment(&experiment).map_err(|e| ToolError::Validation(e.to_string()))?;
    // A run over MCP gets no template values: nothing but configuration
    // and secrets can fill a placeholder.
    let no_vars = std::collections::HashMap::<String, String>::new();
    validate_placeholders(&experiment, &no_vars)
        .map_err(|e| ToolError::Validation(e.to_string()))?;

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
    /// Replace a journal already at `journal_path` (parity with the CLI's
    /// `--force`); without it such a journal is kept and the run refused.
    pub force: bool,
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

/// What `tumult_run_experiment` can do about a journal that is already
/// there.
const RUN_WAYS_OUT: &str = "pass force: true to replace it, or choose another journal_path";

/// The error of a journal that could not be reserved or published, at
/// `stage`. One that is already there says `ways_out`, the arguments of the
/// tool.
pub(super) fn journal_error(
    error: &tumult_core::journal::JournalError,
    stage: &str,
    ways_out: &str,
) -> ToolError {
    if let tumult_core::journal::JournalError::Exists(path) = error {
        return ToolError::AlreadyExists(format!("journal {}; {ways_out}", path.display()));
    }
    ToolError::Execution(format!("{stage}: {error}"))
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
/// journal cannot be encoded or written. A journal already at
/// `journal_path` is [`ToolError::AlreadyExists`] without `force`, before
/// anything runs.
pub fn run_experiment(request: RunExperimentRequest<'_>) -> Result<StructuredReport, ToolError> {
    use std::sync::Arc;
    use tumult_core::controls::{ControlRegistry, ProviderControl};
    use tumult_core::engine::{
        apply_template_vars, build_config_env, build_secret_env, flatten_secrets, parse_experiment,
        resolve_config, resolve_secrets, validate_experiment,
    };
    use tumult_core::journal::{encode_journal, ExistingJournal, JournalOutput};
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

    // Same configuration/secrets semantics as `tumult run`: values resolve
    // up front (a missing env var or secret file fails the call), template
    // substitution applies, and the resolved pairs are injected into
    // provider subprocesses as TUMULT_CONFIG_* / TUMULT_SECRET_*.
    let config = resolve_config(&experiment.configuration)
        .map_err(|e| ToolError::Validation(format!("failed to resolve configuration: {e}")))?;
    let secrets = resolve_secrets(&experiment.secrets)
        .map_err(|e| ToolError::Validation(format!("failed to resolve secrets: {e}")))?;
    let secrets_flat = flatten_secrets(&secrets);
    let experiment = if config.is_empty() && secrets_flat.is_empty() {
        experiment
    } else {
        apply_template_vars(
            &experiment,
            &std::collections::HashMap::new(),
            &config,
            &secrets_flat,
        )
        .map_err(|e| ToolError::Validation(format!("failed to apply template variables: {e}")))?
    };
    validate_experiment(&experiment).map_err(|e| ToolError::Validation(e.to_string()))?;

    // The warnings name keys only, never values: these maps carry resolved
    // secrets, and logs must never see a secret value.
    let (config_env, skipped_config) = build_config_env(&config);
    let (secret_env, skipped_secrets) = build_secret_env(&secrets_flat);
    for key in &skipped_config {
        tracing::warn!(
            name: "mcp.experiment.config_key.skipped",
            { config.key = %key },
            "configuration key does not form a valid env var name after uppercasing; \
             usable in templates but not injected as TUMULT_CONFIG_*"
        );
    }
    for key in &skipped_secrets {
        tracing::warn!(
            name: "mcp.experiment.secret_key.skipped",
            { config.key = %key },
            "secret key does not form a valid env var name after uppercasing; \
             usable in templates but not injected as TUMULT_SECRET_*"
        );
    }
    let mut injected_env = config_env;
    injected_env.extend(secret_env);

    let executor: Arc<dyn ActivityExecutor> = Arc::new(
        crate::handler::ProcessExecutor::with_injected_env(injected_env),
    );

    // Wire experiment-declared controls into the registry so they actually
    // execute at lifecycle events (an empty registry would silently drop
    // them). Declared controls share the run's provider executor.
    let mut controls_registry = ControlRegistry::new();
    for control in &experiment.controls {
        controls_registry.register(Box::new(ProviderControl::new(
            control.clone(),
            executor.clone(),
        )));
    }
    let controls = Arc::new(controls_registry);
    let config = RunConfig {
        version_resolver: None,
        baseline_mode: tumult_core::runner::BaselineMode::default(),
        rollback_strategy: strategy,
        cancellation_token: None,
        parent_context: request.parent_context,
        // The runner runs a declared load with the adapter of its tool.
        load_executor: None,
    };

    let output = JournalOutput::prepare(
        request.journal_path,
        ExistingJournal::replace_if(request.force),
    )
    .map_err(|e| journal_error(&e, "journal preflight failed", RUN_WAYS_OUT))?;
    let journal = run(&experiment, &executor, &controls, &config)
        .map_err(|e| ToolError::Execution(e.to_string()))?;

    // Persist the journal (CLI parity: `tumult run` always writes it).
    output
        .write(&encode_journal(&journal).map_err(|e| ToolError::Execution(e.to_string()))?)
        .map_err(|e| journal_error(&e, "failed to write journal", RUN_WAYS_OUT))?;

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
    let store = tumult_lake::AnalyticsStore::open(Path::new(store_path))
        .map_err(|e| ToolError::Store(e.to_string()))?;
    store
        .ingest_journal_with_experiment(journal, Some(experiment))
        .map_err(|e| ToolError::Store(e.to_string()))
}

/// Create an experiment file from the starter template. With `plugin`,
/// its first step reads a probe of that script plugin.
///
/// # Errors
///
/// Returns [`ToolError::AlreadyExists`] if the file already exists,
/// [`ToolError::InvalidInput`] if no installed script plugin `plugin` has
/// a probe, or [`ToolError::Io`] if the file cannot be written.
pub fn create_experiment(output_path: &str, plugin: Option<&str>) -> Result<String, ToolError> {
    create_experiment_from(output_path, plugin, tumult_authoring::build_catalog)
}

/// [`create_experiment`], with the fault catalog from `catalog`.
fn create_experiment_from<E: std::fmt::Display>(
    output_path: &str,
    plugin: Option<&str>,
    catalog: impl FnOnce() -> Result<tumult_authoring::FaultCatalog, E>,
) -> Result<String, ToolError> {
    let path = Path::new(output_path);
    if path.exists() {
        return Err(ToolError::AlreadyExists(format!(
            "{output_path} already exists"
        )));
    }

    let experiment = match plugin {
        None => tumult_authoring::starter(),
        Some(plugin) => {
            let catalog = catalog().map_err(|e| ToolError::Execution(e.to_string()))?;
            tumult_authoring::starter_for(&catalog, plugin)
        }
    }
    .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
    let toon = tumult_authoring::encode_experiment(&experiment)
        .map_err(|e| ToolError::Execution(e.to_string()))?;

    std::fs::write(path, &toon)?;
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

    /// A `${...}` that nothing fills reaches its step as it stands. The
    /// tool refuses it, as `tumult validate` does.
    #[test]
    fn validate_refuses_a_placeholder_that_nothing_fills() {
        let dir = TempDir::new().unwrap();
        let path = write_valid_experiment(dir.path());
        let text = std::fs::read_to_string(&path).unwrap();
        let drain = text.replacen("MCP test experiment", "drain ${zone_a_node}", 1);
        std::fs::write(&path, drain).unwrap();

        let err = validate_experiment(&path).unwrap_err();

        assert!(matches!(err, ToolError::Validation(_)), "{err}");
        assert!(
            err.to_string().contains("nothing fills ${zone_a_node}"),
            "{err}"
        );
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
            force: false,
            store_path,
            no_ingest,
            format,
            parent_context: None,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn invalid_journal_destination_prevents_method_execution() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("must-not-execute");
        let path = dir.path().join("experiment.toon");
        std::fs::write(&path, format!(
            "title: Journal preflight\nmethod[1]:\n  - name: mark\n    activity_type: action\n    provider:\n      type: process\n      path: touch\n      arguments[1]: {}\nrollbacks[0]:\n", marker.display()
        )).unwrap();
        let result = run_experiment(run_request(
            path.to_str().unwrap(),
            "on-deviation",
            dir.path(),
            "unused",
            true,
            "json",
        ));
        assert!(result.is_err());
        assert!(
            !marker.exists(),
            "fault ran before journal destination was checked"
        );
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
        let store = tumult_lake::AnalyticsStore::open(&store_path).unwrap();
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

    /// End-to-end proof for the config/secrets/controls wiring on the MCP
    /// run path: resolved configuration is injected into provider
    /// subprocesses as `TUMULT_CONFIG_*`, and experiment-declared controls
    /// fire at lifecycle events (previously this path injected nothing and
    /// built an empty registry).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_injects_config_env_and_fires_declared_controls() {
        let dir = TempDir::new().unwrap();
        let events_file = dir.path().join("control-events.txt");
        let exp_path = dir.path().join("wired.toon");
        let experiment = format!(
            r#"title: MCP wiring experiment

tags[1]: test

configuration:
  marker:
    type: inline
    value: mcp-wired

controls[1]:
  - name: event-recorder
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "echo \"$TUMULT_CONTROL_EVENT\" >> {}"
      timeout_s: 5.0

method[1]:
  - name: read-injected-env
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "echo \"cfg=$TUMULT_CONFIG_MARKER\""
      timeout_s: 5.0
"#,
            events_file.display()
        );
        std::fs::write(&exp_path, experiment).unwrap();

        let journal_path = dir.path().join("journal.toon");
        let result = run_experiment(run_request(
            exp_path.to_str().unwrap(),
            "on-deviation",
            &journal_path,
            "unused.duckdb",
            true,
            "json",
        ))
        .unwrap();

        // The resolved config reached the method subprocess as TUMULT_CONFIG_*.
        assert!(
            result.text.contains("cfg=mcp-wired"),
            "TUMULT_CONFIG_* env injection missing from journal: {}",
            result.text
        );

        // The declared control fired at lifecycle events.
        let events = std::fs::read_to_string(&events_file).unwrap();
        for expected in ["before_experiment", "before_method", "after_experiment"] {
            assert!(
                events.contains(expected),
                "declared control did not fire on {expected}; events file: {events}"
            );
        }
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

    /// The catalog of the script plugins shipped in the repository.
    fn shipped_catalog() -> Result<tumult_authoring::FaultCatalog, String> {
        let plugins = Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins");
        let config = tumult_plugin::discovery::PluginDiscoveryConfig {
            plugin_paths: vec![plugins],
        };
        tumult_authoring::build_catalog_with_config(&config).map_err(|e| e.to_string())
    }

    #[test]
    fn create_experiment_with_plugin_first_reads_a_probe_of_it() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("kafka.toon");

        create_experiment_from(
            path.to_str().unwrap(),
            Some("tumult-kafka"),
            shipped_catalog,
        )
        .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let experiment = tumult_core::engine::parse_experiment(&content).unwrap();
        assert!(
            matches!(&experiment.method[0].provider,
                tumult_core::types::Provider::Script { plugin, .. } if plugin == "tumult-kafka"),
            "{content}"
        );
    }

    #[test]
    fn create_experiment_refuses_a_plugin_without_a_probe() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("none.toon");

        let result =
            create_experiment_from(path.to_str().unwrap(), Some("tumult-nope"), shipped_catalog);

        assert!(
            matches!(result, Err(ToolError::InvalidInput(_))),
            "{result:?}"
        );
        assert!(!path.exists());
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
