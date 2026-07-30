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

use tumult_exec::ProviderExecutor;

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
            Ok((true, via)) => println!(
                "Ingested into persistent analytics store{}",
                via.map_or_else(String::new, |url| format!(" via daemon ({url})"))
            ),
            Ok((false, _)) => println!("Already in analytics store (duplicate)"),
            Err(e) => eprintln!("warning: auto-ingest failed: {e}"),
        }
    }

    // Exit with non-zero if experiment did not complete successfully
    if journal.status != ExperimentStatus::Completed {
        bail!("experiment finished with status: {:?}", journal.status);
    }

    Ok(())
}

/// Env var pointing the CLI at a running tumultd: when set, auto-ingest
/// POSTs the journal to the daemon's `/api/import/journal` so the write
/// rides the daemon's single-writer channel instead of racing its
/// `DuckDB` lock.
const DAEMON_URL_ENV: &str = "TUMULT_DAEMON_URL";

/// Why a daemon import attempt did not succeed.
enum DaemonPostError {
    /// No HTTP response at all (connect refused, timeout): the journal
    /// never reached the daemon, so a direct store write cannot double-write.
    Unreachable(String),
    /// The daemon answered but did not ingest: honored as final — retrying
    /// directly could double-write if the daemon persisted despite the
    /// error response.
    Rejected(String),
}

/// Returns `(ingested, daemon_url)`: `daemon_url` is `Some` when the journal
/// went through the daemon (used for the user-facing confirmation line).
async fn auto_ingest_journal(
    journal: &Journal,
    experiment: &Experiment,
) -> Result<(bool, Option<String>)> {
    use tumult_lake::AnalyticsBackend;

    // Dual-mode: ClickHouse if configured, DuckDB otherwise
    if tumult_clickhouse::ClickHouseConfig::is_configured() {
        let config = tumult_clickhouse::ClickHouseConfig::from_env();
        let store = tumult_clickhouse::ClickHouseStore::connect(&config)
            .await
            .context("failed to connect to ClickHouse analytics backend")?;
        let ingested = store.ingest_journal(journal)?;
        return Ok((ingested, None));
    }

    // Daemon-first: a running tumultd holds the store's single-writer lock,
    // so a direct open would fail. Fall back to the direct write ONLY when
    // the daemon never answered (no HTTP response at all).
    if let Ok(base) = std::env::var(DAEMON_URL_ENV) {
        match post_journal_to_daemon(&base, journal, experiment).await {
            Ok(ingested) => return Ok((ingested, Some(base))),
            Err(DaemonPostError::Unreachable(reason)) => {
                eprintln!(
                    "warning: daemon at {base} unreachable ({reason}); writing the store directly"
                );
            }
            Err(DaemonPostError::Rejected(reason)) => {
                bail!("daemon rejected journal import: {reason}");
            }
        }
    }

    // Default: DuckDB embedded
    let db_path = tumult_lake::AnalyticsStore::default_path()
        .context("failed to resolve analytics store path")?;
    let store = tumult_lake::AnalyticsStore::open(&db_path)
        .with_context(|| format!("failed to open analytics store: {}", db_path.display()))?;
    let ingested = store.ingest_journal_with_experiment(journal, Some(experiment))?;

    emit_store_metrics(&db_path, &store);

    Ok((ingested, None))
}

/// POST `{journal, experiment}` to `{base}/api/import/journal`. The daemon
/// dedups on `experiment_id`; the response's `ingested` flag says whether
/// this call wrote anything.
async fn post_journal_to_daemon(
    base: &str,
    journal: &Journal,
    experiment: &Experiment,
) -> std::result::Result<bool, DaemonPostError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| DaemonPostError::Unreachable(e.to_string()))?;
    let url = format!("{}/api/import/journal", base.trim_end_matches('/'));
    let resp = client
        .post(&url)
        .json(&serde_json::json!({"journal": journal, "experiment": experiment}))
        .send()
        .await
        .map_err(|e| DaemonPostError::Unreachable(e.to_string()))?;
    let status = resp.status();
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| DaemonPostError::Rejected(format!("HTTP {status}, unreadable body: {e}")))?;
    if !status.is_success() {
        let msg = body
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown error");
        return Err(DaemonPostError::Rejected(format!("HTTP {status}: {msg}")));
    }
    Ok(body
        .get("ingested")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false))
}

fn emit_store_metrics(db_path: &Path, store: &tumult_lake::AnalyticsStore) {
    let size_bytes = std::fs::metadata(db_path).map(|m| m.len()).ok();
    if let Ok(stats) = store.stats() {
        tumult_lake::telemetry::record_store_gauges(
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Env vars are process-global; serialize every test that touches them.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn test_journal(id: &str) -> Journal {
        Journal {
            experiment_title: format!("Test {id}"),
            experiment_id: id.into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_300_000_000_000,
            duration_ms: 300_000,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![],
            rollback_results: vec![],
            rollback_failures: 0,
            halt: None,
            blast_radius: None,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
        }
    }

    /// An unreachable daemon must fall back to the direct `DuckDB` write: a
    /// connection-level failure means the journal never left the process, so
    /// writing the store directly cannot double-write.
    // The env guard must cover the awaited ingest (env vars are read inside
    // it). Only tests serialized on ENV_MUTEX can block on it, so holding a
    // std guard across the await cannot deadlock the test runtime.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn unreachable_daemon_falls_back_to_direct_write() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("lake.duckdb");
        std::env::set_var("TUMULT_LAKE_PATH", &db_path);
        // Port 1 is never listening: connection refused, no HTTP response.
        std::env::set_var(DAEMON_URL_ENV, "http://127.0.0.1:1");
        std::env::remove_var("TUMULT_CLICKHOUSE_URL");

        let journal = test_journal("fallback-e1");
        let experiment = Experiment::default();
        let (ingested, via) = auto_ingest_journal(&journal, &experiment).await.unwrap();
        assert!(ingested);
        assert_eq!(via, None, "unreachable daemon must not claim the ingest");

        std::env::remove_var(DAEMON_URL_ENV);
        std::env::remove_var("TUMULT_LAKE_PATH");

        let store = tumult_lake::AnalyticsStore::open_read_only(&db_path).unwrap();
        assert_eq!(store.experiment_count().unwrap(), 1);
    }
}
