use std::path::Path;

use anyhow::{bail, Context, Result};

use tumult_core::controls::{ControlRegistry, ProviderControl};
use tumult_core::engine::{
    apply_template_vars, build_config_env, build_secret_env, flatten_secrets, parse_experiment,
    resolve_config, resolve_secrets, validate_experiment,
};
use tumult_core::execution::RollbackStrategy;
use tumult_core::journal::write_journal;
use tumult_core::runner::{run_experiment, RunConfig};
use tumult_core::types::{Experiment, ExperimentStatus, Journal};

use super::exec::ProviderExecutor;
use super::load::K6LoadExecutor;
use super::print_dry_run;

// ── Run command ───────────────────────────────────────────────

/// # Errors
///
/// Returns an error if the experiment cannot be read, parsed, validated,
/// executed, or the journal cannot be written.
#[allow(clippy::too_many_arguments)]
#[must_use = "callers must handle experiment run errors"]
pub async fn cmd_run<S: ::std::hash::BuildHasher>(
    experiment_path: &Path,
    journal_path: &Path,
    force: bool,
    dry_run: bool,
    rollback_strategy: RollbackStrategy,
    auto_ingest: bool,
    vars: std::collections::HashMap<String, String, S>,
    load_override: Option<tumult_core::types::LoadConfig>,
) -> Result<()> {
    // S-C3: File size limit before deserialization (10MB max)
    let file_size = tokio::fs::metadata(experiment_path)
        .await
        .map_or(0, |m| m.len());
    if file_size > 10 * 1024 * 1024 {
        bail!(
            "experiment file too large ({} bytes, max 10MB): {}",
            file_size,
            experiment_path.display()
        );
    }

    // A journal holds the evidence of exactly one run: silently overwriting
    // it destroys the previous run's record. Refuse unless --force was given.
    // Checked up front (not just before writing) so a doomed run does not
    // execute faults first; a dry run writes no journal and skips the check.
    if !dry_run && journal_path.exists() && !force {
        bail!(
            "journal already exists: {} — pass --force to overwrite, or choose \
             a different --journal-path",
            journal_path.display()
        );
    }

    let content = tokio::fs::read_to_string(experiment_path)
        .await
        .with_context(|| {
            format!(
                "failed to read experiment file: {}",
                experiment_path.display()
            )
        })?;

    let experiment = parse_experiment(&content)
        .with_context(|| format!("failed to parse experiment: {}", experiment_path.display()))?;

    // Resolve configuration and secrets first: the resolved values feed both
    // template substitution (${config.*} / ${secrets.*}) and the subprocess
    // env injection below. A missing env var or secret file is fatal here,
    // before anything executes.
    let config = resolve_config(&experiment.configuration)?;
    let secrets = resolve_secrets(&experiment.secrets)?;
    let secrets_flat = flatten_secrets(&secrets);

    // Apply template substitution when any placeholder source exists: --var
    // values plus the ${config.<name>} / ${secrets.<group>.<key>} namespaces.
    // $${...} escapes to a literal ${...} for shell-style text.
    let mut experiment = if vars.is_empty() && config.is_empty() && secrets_flat.is_empty() {
        experiment
    } else {
        apply_template_vars(&experiment, &vars, &config, &secrets_flat)
            .with_context(|| "failed to apply template variables to experiment")?
    };

    validate_experiment(&experiment)?;

    if dry_run {
        print_dry_run(&experiment);
        return Ok(());
    }

    // Build the TUMULT_CONFIG_* / TUMULT_SECRET_* environment injected into
    // process and script provider subprocesses. Keys that cannot form valid
    // env var names are skipped with a warning — the warnings name keys
    // only, never values: these maps carry resolved secrets, and journals,
    // logs, and analytics must never see a secret value.
    let (config_env, skipped_config) = build_config_env(&config);
    let (secret_env, skipped_secrets) = build_secret_env(&secrets_flat);
    for key in &skipped_config {
        eprintln!(
            "warning: configuration key '{key}' does not form a valid env var name after \
             uppercasing; usable in templates but not injected as TUMULT_CONFIG_*"
        );
    }
    for key in &skipped_secrets {
        eprintln!(
            "warning: secret key '{key}' does not form a valid env var name after \
             uppercasing; usable in templates but not injected as TUMULT_SECRET_*"
        );
    }
    let mut injected_env = config_env;
    injected_env.extend(secret_env);

    let executor = ProviderExecutor::with_injected_env(injected_env);
    let executor_arc: std::sync::Arc<dyn tumult_core::runner::ActivityExecutor> =
        std::sync::Arc::new(executor);

    // Wire experiment-declared controls into the registry so they actually
    // execute at lifecycle events (an empty registry would silently drop
    // them). Declared controls share the run's provider executor.
    let mut controls = ControlRegistry::new();
    for control in &experiment.controls {
        controls.register(Box::new(ProviderControl::new(
            control.clone(),
            executor_arc.clone(),
        )));
    }
    let controls_arc = std::sync::Arc::new(controls);

    // Spawn a task that cancels the experiment if SIGINT (Ctrl-C) is received.
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let cancel_token_for_signal = cancel_token.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::warn!("SIGINT received — cancelling experiment");
            cancel_token_for_signal.cancel();
        }
    });

    // Apply load override from CLI flags, or use experiment's load config
    if let Some(ref override_config) = load_override {
        experiment.load = Some(override_config.clone());
    }

    // Create K6 load executor if experiment has a load config
    let load_executor: Option<std::sync::Arc<dyn tumult_core::runner::LoadExecutor>> =
        if experiment.load.is_some() {
            Some(std::sync::Arc::new(K6LoadExecutor))
        } else {
            None
        };

    let run_config = RunConfig {
        rollback_strategy,
        cancellation_token: Some(cancel_token),
        parent_context: None,
        load_executor,
        max_concurrent_faults: None,
    };

    println!("Running experiment: {}", experiment.title);

    let journal = run_experiment(&experiment, &executor_arc, &controls_arc, &run_config)?;

    write_journal(&journal, journal_path)?;

    println!("Status: {:?}", journal.status);
    println!("Duration: {}ms", journal.duration_ms);
    println!("Method steps: {} executed", journal.method_results.len());
    if !journal.rollback_results.is_empty() {
        println!("Rollbacks: {} executed", journal.rollback_results.len());
    }
    println!("Journal written to: {}", journal_path.display());

    // Auto-ingest into persistent analytics store
    if auto_ingest {
        match auto_ingest_journal(&journal, &experiment).await {
            Ok(true) => println!("Ingested into persistent analytics store"),
            Ok(false) => println!("Already in analytics store (duplicate)"),
            Err(e) => eprintln!("warning: auto-ingest failed: {e}"),
        }
    }

    // Exit with non-zero if experiment did not complete successfully
    if journal.status != ExperimentStatus::Completed {
        bail!("experiment finished with status: {:?}", journal.status);
    }

    Ok(())
}

async fn auto_ingest_journal(journal: &Journal, experiment: &Experiment) -> Result<bool> {
    use tumult_analytics::AnalyticsBackend;

    // Dual-mode: ClickHouse if configured, DuckDB otherwise
    if tumult_clickhouse::ClickHouseConfig::is_configured() {
        let config = tumult_clickhouse::ClickHouseConfig::from_env();
        let store = tumult_clickhouse::ClickHouseStore::connect(&config)
            .await
            .context("failed to connect to ClickHouse analytics backend")?;
        let ingested = store.ingest_journal(journal)?;
        return Ok(ingested);
    }

    // Default: DuckDB embedded
    let db_path = tumult_analytics::AnalyticsStore::default_path()
        .context("failed to resolve analytics store path")?;
    let store = tumult_analytics::AnalyticsStore::open(&db_path)
        .with_context(|| format!("failed to open analytics store: {}", db_path.display()))?;
    let ingested = store.ingest_journal_with_experiment(journal, Some(experiment))?;

    emit_store_metrics(&db_path, &store);

    Ok(ingested)
}

fn emit_store_metrics(db_path: &Path, store: &tumult_analytics::AnalyticsStore) {
    let size_bytes = std::fs::metadata(db_path).map(|m| m.len()).ok();
    if let Ok(stats) = store.stats() {
        tumult_analytics::telemetry::record_store_gauges(
            stats.experiment_count,
            stats.activity_count,
            size_bytes,
        );
    }

    // Disk usage percentage via df (Unix only)
    #[cfg(unix)]
    if let Some(parent) = db_path.parent() {
        if let Ok(output) = std::process::Command::new("df")
            .arg("-k")
            .arg(parent)
            .output()
        {
            if let Ok(stdout) = String::from_utf8(output.stdout) {
                if let Some(line) = stdout.lines().nth(1) {
                    let fields: Vec<&str> = line.split_whitespace().collect();
                    if fields.len() >= 5 {
                        if let Ok(pct) = fields[4].trim_end_matches('%').parse::<u64>() {
                            let meter = opentelemetry::global::meter("tumult-analytics");
                            let gauge = meter.u64_gauge("tumult.store.disk_usage_pct").build();
                            gauge.record(
                                pct,
                                &[opentelemetry::KeyValue::new(
                                    "tumult.store.path",
                                    db_path.display().to_string(),
                                )],
                            );
                        }
                    }
                }
            }
        }
    }
}
