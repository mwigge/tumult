//! Tests for import/export, auto-ingest on run, and the store subcommands.

use super::super::*;
use super::helpers::*;
use tempfile::TempDir;
use tumult_core::execution::RollbackStrategy;

// ── Phase 4: Import/Export roundtrip ──────────────────────

#[test]
fn import_rejects_missing_directory() {
    let result = cmd_import(Path::new("/nonexistent/path"));
    assert!(result.is_err());
}

#[test]
fn import_rejects_missing_parquet_files() {
    let d = TempDir::new().unwrap();
    let result = cmd_import(d.path());
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("experiments.parquet not found"));
}

// ── Phase 4: Run with auto-ingest ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cmd_run_with_auto_ingest() {
    let d = TempDir::new().unwrap();
    let exp_path = write_valid_experiment(d.path());
    let journal_path = d.path().join("out.toon");

    // Run with auto-ingest disabled (avoids touching real ~/.tumult)
    let result = cmd_run(
        &exp_path,
        &journal_path,
        false,
        false,
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await;
    assert!(result.is_ok());
    assert!(journal_path.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cmd_run_dry_run_does_not_ingest() {
    let d = TempDir::new().unwrap();
    let exp_path = write_valid_experiment(d.path());
    let journal_path = d.path().join("out.toon");

    let result = cmd_run(
        &exp_path,
        &journal_path,
        false,
        true,
        RollbackStrategy::OnDeviation,
        true,
        std::collections::HashMap::new(),
        None,
    )
    .await;
    assert!(result.is_ok());
    // Journal should NOT be written in dry-run mode
    assert!(!journal_path.exists());
}

// ── Phase 4: Store command tests ──────────────────────────

#[test]
fn store_backup_creates_parquet_files() {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::types::*;

    let d = TempDir::new().unwrap();
    let db_path = d.path().join("test.duckdb");
    let backup_dir = d.path().join("backup");

    // Create store with data
    let store = AnalyticsStore::open(&db_path).unwrap();
    store
        .ingest_journal(&Journal {
            experiment_title: "test".into(),
            experiment_id: "e1".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_060_000_000_000,
            duration_ms: 60_000,
            method_results: vec![],
            steady_state_before: None,
            steady_state_after: None,
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
        })
        .unwrap();
    drop(store);

    // Backup via store API directly
    let store = AnalyticsStore::open(&db_path).unwrap();
    std::fs::create_dir_all(&backup_dir).unwrap();
    store
        .export_tables(
            &backup_dir.join("experiments.parquet"),
            &backup_dir.join("activities.parquet"),
        )
        .unwrap();

    assert!(backup_dir.join("experiments.parquet").exists());
    assert!(backup_dir.join("activities.parquet").exists());
}

#[test]
fn store_purge_removes_old_data() {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::types::*;

    let d = TempDir::new().unwrap();
    let db_path = d.path().join("test.duckdb");
    let store = AnalyticsStore::open(&db_path).unwrap();

    // Old experiment (2020)
    store
        .ingest_journal(&Journal {
            experiment_title: "old".into(),
            experiment_id: "old-1".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1_577_836_800_000_000_000,
            ended_at_ns: 1_577_836_860_000_000_000,
            duration_ms: 60_000,
            method_results: vec![],
            steady_state_before: None,
            steady_state_after: None,
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
        })
        .unwrap();

    // Recent experiment
    let recent_started_at_ns = chrono::Utc::now()
        .timestamp_nanos_opt()
        .unwrap_or(i64::MAX - 60_000_000_000);
    store
        .ingest_journal(&Journal {
            experiment_title: "new".into(),
            experiment_id: "new-1".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: recent_started_at_ns,
            ended_at_ns: recent_started_at_ns.saturating_add(60_000_000_000),
            duration_ms: 60_000,
            method_results: vec![],
            steady_state_before: None,
            steady_state_after: None,
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
        })
        .unwrap();

    assert_eq!(store.experiment_count().unwrap(), 2);
    let purged = store.purge_older_than_days(30).unwrap();
    assert_eq!(purged, 1);
    assert_eq!(store.experiment_count().unwrap(), 1);
}

#[test]
fn store_stats_reports_counts() {
    use tumult_analytics::AnalyticsStore;
    use tumult_core::types::*;

    let store = AnalyticsStore::in_memory().unwrap();
    let stats = store.stats().unwrap();
    assert_eq!(stats.experiment_count, 0);
    assert_eq!(stats.activity_count, 0);

    store
        .ingest_journal(&Journal {
            experiment_title: "test".into(),
            experiment_id: "e1".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_060_000_000_000,
            duration_ms: 60_000,
            method_results: vec![ActivityResult {
                name: "act".into(),
                activity_type: ActivityType::Action,
                status: ActivityStatus::Succeeded,
                started_at_ns: 1_774_980_000_000_000_000,
                duration_ms: 500,
                output: None,
                error: None,
                trace_id: TraceId::empty(),
                span_id: SpanId::empty(),
            }],
            steady_state_before: None,
            steady_state_after: None,
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
        })
        .unwrap();

    let stats = store.stats().unwrap();
    assert_eq!(stats.experiment_count, 1);
    assert_eq!(stats.activity_count, 1);
}
