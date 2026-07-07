use std::path::Path;

use anyhow::{bail, Context, Result};

use tumult_core::controls::ControlRegistry;
use tumult_core::engine::{
    apply_vars, parse_experiment, resolve_config, resolve_secrets, validate_experiment,
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

    // Apply template variable substitution if any --var flags were provided.
    let mut experiment = if vars.is_empty() {
        experiment
    } else {
        apply_vars(&experiment, &vars)
            .with_context(|| "failed to apply template variables to experiment")?
    };

    validate_experiment(&experiment)?;

    // Resolve configuration and secrets
    let _config = resolve_config(&experiment.configuration)?;
    let _secrets = resolve_secrets(&experiment.secrets)?;

    if dry_run {
        print_dry_run(&experiment);
        return Ok(());
    }

    let executor = ProviderExecutor;
    let controls = ControlRegistry::new();

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

    let executor_arc: std::sync::Arc<dyn tumult_core::runner::ActivityExecutor> =
        std::sync::Arc::new(executor);
    let controls_arc = std::sync::Arc::new(controls);
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
    let db_path = tumult_analytics::AnalyticsStore::default_path();
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
