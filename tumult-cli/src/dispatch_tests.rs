//! Tests for the dispatch wiring in `main.rs`: each test parses an argv
//! through the real clap definitions (so flag/value mapping is exercised) and
//! runs the resulting command through `dispatch`, asserting the observable
//! behavior the command handlers guarantee — files written, stores populated,
//! and clean errors for bad input. Commands that need a live service (MCP
//! server, TUI) are not exercised here.

use std::path::{Path, PathBuf};

use clap::{CommandFactory, Parser};
use tempfile::TempDir;

use crate::cli::Cli;
use crate::dispatch;

/// Env vars (`TUMULT_LAKE_PATH`, `HOME`) and the process working directory
/// are process-global; every test that touches either holds this lock for its
/// whole body so concurrent tests cannot observe each other's overrides. The
/// binary's test process is separate from the library's, so this lock only
/// needs to cover the tests in this module.
static GLOBAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Parse argv through the real clap definitions and dispatch the command.
///
/// Uses `try_parse_from` so a malformed argv surfaces as a normal `Err` —
/// `parse_from` would exit the whole test process on a parse failure.
async fn run_argv(args: &[&str]) -> anyhow::Result<()> {
    let cli = Cli::try_parse_from(args).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    dispatch(cli.command).await
}

// ── Fixtures ─────────────────────────────────────────────────

const EXPERIMENT_TOON: &str = r#"title: dispatch test experiment
description: exercises the CLI dispatch

tags[1]: test

method[1]:
  - name: echo-action
    activity_type: action
    provider:
      type: process
      path: echo
      arguments[1]: "hello"
      timeout_s: 5.0
"#;

fn write_experiment(dir: &Path) -> PathBuf {
    let path = dir.join("exp.toon");
    std::fs::write(&path, EXPERIMENT_TOON).unwrap();
    path
}

fn test_journal(id: &str) -> tumult_core::types::Journal {
    use tumult_core::types::*;
    Journal {
        experiment_title: format!("dispatch {id}"),
        experiment_id: id.into(),
        status: ExperimentStatus::Completed,
        started_at_ns: 1_774_980_000_000_000_000,
        ended_at_ns: 1_774_980_060_000_000_000,
        duration_ms: 60_000,
        steady_state_before: None,
        steady_state_after: None,
        method_results: vec![ActivityResult {
            name: "step".into(),
            activity_type: ActivityType::Action,
            status: ActivityStatus::Succeeded,
            started_at_ns: 1_774_980_000_000_000_000,
            duration_ms: 500,
            output: Some("ok".into()),
            error: None,
            trace_id: TraceId::empty(),
            span_id: SpanId::empty(),
        }],
        rollback_results: vec![],
        rollback_failures: 0,
        estimate: None,
        baseline_result: None,
        during_result: None,
        post_result: None,
        load_result: None,
        analysis: None,
        regulatory: None,
        halt: None,
        blast_radius: None,
    }
}

fn write_journal(dir: &Path, id: &str) -> PathBuf {
    let path = dir.join(format!("{id}.toon"));
    tumult_core::journal::write_journal(&test_journal(id), &path).unwrap();
    path
}

/// Create a populated analytics store at `db` and close it, so command
/// handlers can reopen it.
fn populate_store(db: &Path, ids: &[&str]) {
    let store = tumult_lake::AnalyticsStore::open(db).unwrap();
    for id in ids {
        store.ingest_journal(&test_journal(id)).unwrap();
    }
}

/// Point the persistent analytics store at a temp file. Caller must hold
/// [`GLOBAL_LOCK`].
fn use_temp_store(dir: &Path) -> PathBuf {
    let db = dir.join("lake.duckdb");
    std::env::set_var("TUMULT_LAKE_PATH", &db);
    db
}

/// RAII guard that switches the process cwd and restores it on drop.
struct CwdGuard {
    prev: PathBuf,
}

impl CwdGuard {
    fn enter(dir: &Path) -> Self {
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir).unwrap();
        Self { prev }
    }
}

impl Drop for CwdGuard {
    fn drop(&mut self) {
        std::env::set_current_dir(&self.prev).unwrap();
    }
}

// ── Parser-level sanity ──────────────────────────────────────

#[test]
fn cli_definition_passes_clap_debug_assert() {
    // Validates every derive invariant (unique names, valid defaults,
    // required-unless relations, …) across all subcommand modules.
    Cli::command().debug_assert();
}

// ── validate / discover / init / new / templates ─────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_validate_ok_and_missing_file_errors() {
    let dir = TempDir::new().unwrap();
    let exp = write_experiment(dir.path());
    let exp_arg = exp.to_str().unwrap();

    run_argv(&["tumult", "validate", exp_arg]).await.unwrap();

    let missing = dir.path().join("missing.toon");
    let err = run_argv(&["tumult", "validate", missing.to_str().unwrap()])
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("failed to read experiment file"),
        "{err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_discover_lists_and_unknown_filter_errors() {
    run_argv(&["tumult", "discover"]).await.unwrap();

    let err = run_argv(&["tumult", "discover", "--plugin", "no-such-plugin"])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");
}

// The lock must cover the awaited dispatch (cwd is read inside it). Only
// tests serialized on GLOBAL_LOCK can block on it, so holding a std guard
// across the await cannot deadlock the test runtime.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_init_scaffolds_template_in_cwd() {
    let _guard = GLOBAL_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let _cwd = CwdGuard::enter(dir.path());

    run_argv(&["tumult", "init", "--plugin", "tumult-pg"])
        .await
        .unwrap();

    let content = std::fs::read_to_string(dir.path().join("experiment.toon")).unwrap();
    assert!(content.contains("tumult-pg"), "{content}");
    tumult_core::engine::parse_experiment(&content).unwrap();

    // A second init refuses to clobber the scaffold.
    let err = run_argv(&["tumult", "init"]).await.unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_templates_and_new() {
    let dir = TempDir::new().unwrap();
    run_argv(&["tumult", "templates"]).await.unwrap();

    let out = dir.path().join("cpu.toon");
    run_argv(&[
        "tumult",
        "new",
        "--from",
        "cpu-stress",
        "--set",
        "target=staging-host",
        "--out",
        out.to_str().unwrap(),
    ])
    .await
    .unwrap();

    let content = std::fs::read_to_string(&out).unwrap();
    assert!(content.contains("staging-host"), "{content}");
    tumult_core::engine::parse_experiment(&content).unwrap();
}

// ── run ──────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_run_dry_run_covers_every_rollback_strategy() {
    let dir = TempDir::new().unwrap();
    let exp = write_experiment(dir.path());
    let exp_arg = exp.to_str().unwrap();

    for strategy in ["always", "on-deviation", "never"] {
        run_argv(&[
            "tumult",
            "run",
            exp_arg,
            "--dry-run",
            "--rollback-strategy",
            strategy,
            "--var",
            "target=localhost",
        ])
        .await
        .unwrap();
    }
}

#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_run_executes_and_prints_json_journal() {
    let _guard = GLOBAL_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    use_temp_store(dir.path());
    std::env::remove_var("TUMULT_CLICKHOUSE_URL");
    std::env::remove_var("TUMULT_DAEMON_URL");
    let exp = write_experiment(dir.path());
    let journal = dir.path().join("journal.toon");

    run_argv(&[
        "tumult",
        "run",
        exp.to_str().unwrap(),
        "--journal-path",
        journal.to_str().unwrap(),
        "--output-format",
        "json",
    ])
    .await
    .unwrap();

    std::env::remove_var("TUMULT_LAKE_PATH");

    // The journal exists and is a completed experiment record.
    let journal: tumult_core::types::Journal =
        toon_format::decode_default(&std::fs::read_to_string(&journal).unwrap()).unwrap();
    assert_eq!(
        journal.status,
        tumult_core::types::ExperimentStatus::Completed
    );
}

// ── analyze / trend / report / export / compliance ───────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_analyze_summary_and_raw_query() {
    let dir = TempDir::new().unwrap();
    write_journal(dir.path(), "a-1");
    write_journal(dir.path(), "a-2");
    let dir_arg = dir.path().to_str().unwrap();

    run_argv(&["tumult", "analyze", dir_arg, "--last", "5"])
        .await
        .unwrap();

    run_argv(&[
        "tumult",
        "analyze",
        dir_arg,
        "--query",
        "SELECT status, count(*) FROM experiments GROUP BY status",
    ])
    .await
    .unwrap();

    let err = run_argv(&[
        "tumult",
        "analyze",
        dir_arg,
        "--query",
        "DROP TABLE experiments",
    ])
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("only SELECT/WITH queries are allowed"),
        "{err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_trend_ok_and_unknown_metric_errors() {
    let dir = TempDir::new().unwrap();
    write_journal(dir.path(), "t-1");
    let dir_arg = dir.path().to_str().unwrap();

    run_argv(&["tumult", "trend", dir_arg, "--metric", "duration_ms"])
        .await
        .unwrap();

    let err = run_argv(&["tumult", "trend", dir_arg, "--metric", "nonsense"])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown metric"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_report_writes_html() {
    let dir = TempDir::new().unwrap();
    let journal = write_journal(dir.path(), "r-1");
    let out = dir.path().join("report.html");

    run_argv(&[
        "tumult",
        "report",
        journal.to_str().unwrap(),
        "--output",
        out.to_str().unwrap(),
    ])
    .await
    .unwrap();

    let html = std::fs::read_to_string(&out).unwrap();
    assert!(html.contains("Tumult Experiment Report"), "{html}");
}

#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_export_json_and_csv_into_cwd() {
    let _guard = GLOBAL_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let journal = write_journal(dir.path(), "exp");
    let _cwd = CwdGuard::enter(dir.path());

    run_argv(&[
        "tumult",
        "export",
        journal.to_str().unwrap(),
        "--format",
        "json",
    ])
    .await
    .unwrap();
    // JSON export round-trips back into the same journal.
    let json: tumult_core::types::Journal =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("exp.json")).unwrap())
            .unwrap();
    assert_eq!(json, test_journal("exp"));

    run_argv(&[
        "tumult",
        "export",
        journal.to_str().unwrap(),
        "--format",
        "csv",
    ])
    .await
    .unwrap();
    let csv = std::fs::read_to_string(dir.path().join("exp.csv")).unwrap();
    assert!(csv.contains("exp"), "{csv}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_compliance_report_and_sources() {
    let dir = TempDir::new().unwrap();
    let journal = write_journal(dir.path(), "c-1");

    run_argv(&[
        "tumult",
        "compliance",
        journal.to_str().unwrap(),
        "--framework",
        "soc2",
    ])
    .await
    .unwrap();

    // --sources lists the citation registry without journals.
    run_argv(&["tumult", "compliance", "--framework", "dora", "--sources"])
        .await
        .unwrap();
}

// ── store / import ───────────────────────────────────────────

#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_store_lifecycle() {
    let _guard = GLOBAL_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let db = use_temp_store(dir.path());

    // Missing store: stats/path report absence, backup errors.
    run_argv(&["tumult", "store", "stats"]).await.unwrap();
    run_argv(&["tumult", "store", "path"]).await.unwrap();
    let backup = dir.path().join("backup");
    let err = run_argv(&[
        "tumult",
        "store",
        "backup",
        "--output",
        backup.to_str().unwrap(),
    ])
    .await
    .unwrap_err();
    assert!(
        err.to_string().contains("no persistent store found"),
        "{err}"
    );

    // Populated store: stats, path, backup, purge all succeed.
    populate_store(&db, &["s-1", "s-2"]);
    run_argv(&["tumult", "store", "stats"]).await.unwrap();
    run_argv(&["tumult", "store", "path"]).await.unwrap();
    run_argv(&[
        "tumult",
        "store",
        "backup",
        "--output",
        backup.to_str().unwrap(),
    ])
    .await
    .unwrap();
    assert!(backup.join("experiments.parquet").exists());

    // The fixture journals are dated months in the past, so a 30-day purge
    // removes them all.
    run_argv(&["tumult", "store", "purge", "--older-than-days", "30"])
        .await
        .unwrap();
    {
        let store = tumult_lake::AnalyticsStore::open_read_only(&db).unwrap();
        assert_eq!(store.experiment_count().unwrap(), 0);
    }

    // Importing the backup restores both experiments.
    run_argv(&["tumult", "import", backup.to_str().unwrap()])
        .await
        .unwrap();

    std::env::remove_var("TUMULT_LAKE_PATH");

    let store = tumult_lake::AnalyticsStore::open_read_only(&db).unwrap();
    assert_eq!(store.experiment_count().unwrap(), 2);
}

#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_store_migrate_requires_clickhouse_url() {
    let _guard = GLOBAL_LOCK.lock().unwrap();
    std::env::remove_var("TUMULT_CLICKHOUSE_URL");

    let err = run_argv(&["tumult", "store", "migrate"]).await.unwrap_err();
    assert!(
        err.to_string().contains("TUMULT_CLICKHOUSE_URL not set"),
        "{err}"
    );
}

#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_store_import_legacy_without_sources_errors() {
    let _guard = GLOBAL_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    std::env::remove_var("TUMULT_ANALYTICS_PATH");
    std::env::remove_var("KRONIKA_DB");
    let original_home = std::env::var_os("HOME");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);

    let err = run_argv(&["tumult", "store", "import-legacy"])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no legacy stores found"), "{err}");

    match original_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_import_rejects_incomplete_backup() {
    let dir = TempDir::new().unwrap();
    let err = run_argv(&["tumult", "import", dir.path().to_str().unwrap()])
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("experiments.parquet not found"),
        "{err}"
    );
}

// ── recommend / agents ───────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_recommend_against_populated_store() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("lake.duckdb");
    populate_store(&db, &["rec-1", "rec-2"]);

    run_argv(&[
        "tumult",
        "recommend",
        "--goal",
        "improve postgres resilience",
        "--store-path",
        db.to_str().unwrap(),
        "--format",
        "json",
    ])
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_agents_lists_adapters() {
    run_argv(&["tumult", "agents"]).await.unwrap();
}

// ── agentic ──────────────────────────────────────────────────

#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_agentic_deterministic_paths() {
    let _guard = GLOBAL_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let db = use_temp_store(dir.path());

    run_argv(&["tumult", "agentic", "list-packs"])
        .await
        .unwrap();

    let smoke_journal = dir.path().join("smoke.json");
    run_argv(&[
        "tumult",
        "agentic",
        "smoke",
        "--journal",
        smoke_journal.to_str().unwrap(),
    ])
    .await
    .unwrap();
    assert!(smoke_journal.exists());

    let run_journal = dir.path().join("run.json");
    run_argv(&[
        "tumult",
        "agentic",
        "run",
        "--scenario",
        "malformed-json-recovery",
        "--journal",
        run_journal.to_str().unwrap(),
    ])
    .await
    .unwrap();
    assert!(run_journal.exists());

    let trajectory_journal = dir.path().join("trajectory.json");
    run_argv(&[
        "tumult",
        "agentic",
        "trajectory",
        "--pack",
        "rag-grounding-failure",
        "--journal",
        trajectory_journal.to_str().unwrap(),
    ])
    .await
    .unwrap();
    assert!(trajectory_journal.exists());

    let fixture_path = dir.path().join("fixture.json");
    let fixture = tumult_agentic::replay::complete_replay_fixture();
    std::fs::write(
        &fixture_path,
        serde_json::to_string_pretty(&fixture).unwrap(),
    )
    .unwrap();
    let replay_journal = dir.path().join("replay.json");
    run_argv(&[
        "tumult",
        "agentic",
        "replay",
        "--fixture",
        fixture_path.to_str().unwrap(),
        "--journal",
        replay_journal.to_str().unwrap(),
    ])
    .await
    .unwrap();

    std::env::remove_var("TUMULT_LAKE_PATH");

    // The deterministic runs were ingested into the store (runs sharing a
    // scenario dedupe on the natural key, so only a lower bound is stable;
    // the lib tests pin the exact ingest semantics).
    let store = tumult_lake::AnalyticsStore::open_read_only(&db).unwrap();
    let rows = store.query("SELECT count(*) FROM agentic_runs").unwrap();
    let count: i64 = rows[0][0].parse().unwrap();
    assert!(count >= 1, "agentic runs must be ingested, got {count}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_agentic_live_paths_validate_scenario_names() {
    let err = run_argv(&[
        "tumult",
        "agentic",
        "proxy",
        "--listen",
        "127.0.0.1:0",
        "--upstream",
        "http://upstream.invalid",
        "--scenario",
        "no-such-pack",
    ])
    .await
    .unwrap_err();
    assert!(err.to_string().contains("unknown scenario pack"), "{err}");

    let err = run_argv(&[
        "tumult",
        "agentic",
        "run-live",
        "--prompt",
        "say hi",
        "--scenario",
        "no-such-pack",
    ])
    .await
    .unwrap_err();
    assert!(err.to_string().contains("unknown scenario pack"), "{err}");
}

// ── gameday ──────────────────────────────────────────────────

#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_gameday_create_run_analyze() {
    let _guard = GLOBAL_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    write_experiment(dir.path());
    let _cwd = CwdGuard::enter(dir.path());

    // Create scaffolds <name>.gameday.toon in the working directory.
    run_argv(&[
        "tumult",
        "gameday",
        "create",
        "dispatch-gd",
        "--experiments",
        "exp.toon",
    ])
    .await
    .unwrap();
    let gameday = dir.path().join("dispatch-gd.gameday.toon");
    assert!(gameday.exists());

    // Run executes the experiment and writes <name>.journal.toon.
    run_argv(&["tumult", "gameday", "run", gameday.to_str().unwrap()])
        .await
        .unwrap();
    let journal = gameday.with_extension("journal.toon");
    assert!(journal.exists());

    // Analyze renders the completed journal.
    run_argv(&["tumult", "gameday", "analyze", gameday.to_str().unwrap()])
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_gameday_create_rejects_traversal_name() {
    let err = run_argv(&["tumult", "gameday", "create", "../escape"])
        .await
        .unwrap_err();
    assert!(err.to_string().contains("invalid gameday name"), "{err}");
}

// ── chaosgraph / topology / autopilot (error boundaries) ─────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_chaosgraph_query_missing_store_errors() {
    let missing = Path::new("/nonexistent/dispatch-test.duckdb");
    let err = run_argv(&[
        "tumult",
        "chaosgraph",
        "query",
        "--kind",
        "experiment",
        "--store",
        missing.to_str().unwrap(),
    ])
    .await
    .unwrap_err();
    assert!(err.to_string().contains("store not found"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_topology_import_missing_file_errors() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("lake.duckdb");
    populate_store(&db, &["topo-1"]);
    let err = run_argv(&[
        "tumult",
        "topology",
        "import",
        dir.path().join("missing.toml").to_str().unwrap(),
        "--store",
        db.to_str().unwrap(),
    ])
    .await
    .unwrap_err();
    // The missing topology file surfaces as a clean NotFound error.
    assert!(
        err.to_string().contains("cannot read topology file"),
        "{err}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dispatch_autopilot_status_missing_store_errors() {
    let missing = Path::new("/nonexistent/dispatch-test.duckdb");
    let err = run_argv(&[
        "tumult",
        "autopilot",
        "status",
        "--store",
        missing.to_str().unwrap(),
    ])
    .await
    .unwrap_err();
    assert!(err.to_string().contains("store not found"), "{err}");
}
