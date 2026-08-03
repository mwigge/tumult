//! Tests for the `tumult store` subcommands (stats, backup, purge, path,
//! import-legacy, migrate) and the `tumult import` happy path, all against
//! temp analytics stores.

use super::super::*;
use super::helpers::{use_temp_store, ENV_LOCK};
use tempfile::TempDir;
use tumult_core::types::{ExperimentStatus, Journal};

fn journal(id: &str, started_at_ns: i64) -> Journal {
    Journal {
        experiment_title: format!("store test {id}"),
        experiment_id: id.into(),
        status: ExperimentStatus::Completed,
        started_at_ns,
        ended_at_ns: started_at_ns + 60_000_000_000,
        duration_ms: 60_000,
        steady_state_before: None,
        steady_state_after: None,
        method_results: vec![],
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

/// Create a populated store at `db_path` and close it, so command handlers
/// can reopen it.
fn populated_store(db_path: &std::path::Path, ids: &[&str]) {
    let store = tumult_lake::AnalyticsStore::open(db_path).unwrap();
    for (i, id) in ids.iter().enumerate() {
        #[allow(clippy::cast_possible_wrap)]
        let started = 1_774_980_000_000_000_000 + i as i64 * 1_000_000_000;
        store.ingest_journal(&journal(id, started)).unwrap();
    }
}

// ── stats ────────────────────────────────────────────────────

#[test]
fn store_stats_without_store_reports_absence() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    use_temp_store(dir.path()); // not created on disk

    cmd_store_stats().unwrap();

    std::env::remove_var("TUMULT_LAKE_PATH");
}

#[test]
fn store_stats_reports_schema_and_counts() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let db = use_temp_store(dir.path());
    populated_store(&db, &["s-1", "s-2"]);

    cmd_store_stats().unwrap();

    std::env::remove_var("TUMULT_LAKE_PATH");

    let store = tumult_lake::AnalyticsStore::open_read_only(&db).unwrap();
    assert_eq!(store.experiment_count().unwrap(), 2);
}

// ── backup ───────────────────────────────────────────────────

#[test]
fn store_backup_requires_existing_store() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    use_temp_store(dir.path()); // not created on disk

    let err = cmd_store_backup(&dir.path().join("backup")).unwrap_err();
    assert!(
        err.to_string().contains("no persistent store found"),
        "{err}"
    );

    std::env::remove_var("TUMULT_LAKE_PATH");
}

#[test]
fn store_backup_writes_both_parquet_files() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let db = use_temp_store(dir.path());
    populated_store(&db, &["b-1"]);
    let backup = dir.path().join("backup");

    cmd_store_backup(&backup).unwrap();

    assert!(backup.join("experiments.parquet").exists());
    assert!(backup.join("activities.parquet").exists());

    std::env::remove_var("TUMULT_LAKE_PATH");
}

// ── purge ────────────────────────────────────────────────────

#[test]
fn store_purge_requires_existing_store() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    use_temp_store(dir.path());

    let err = cmd_store_purge(30).unwrap_err();
    assert!(
        err.to_string().contains("no persistent store found"),
        "{err}"
    );

    std::env::remove_var("TUMULT_LAKE_PATH");
}

#[test]
fn store_purge_removes_only_old_experiments() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let db = use_temp_store(dir.path());
    let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
    // 2020 — older than any sane purge window.
    store
        .ingest_journal(&journal("old-1", 1_577_836_800_000_000_000))
        .unwrap();
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    store.ingest_journal(&journal("new-1", now_ns)).unwrap();
    drop(store);

    cmd_store_purge(30).unwrap();

    std::env::remove_var("TUMULT_LAKE_PATH");

    let store = tumult_lake::AnalyticsStore::open_read_only(&db).unwrap();
    assert_eq!(store.experiment_count().unwrap(), 1);
}

#[test]
fn store_purge_with_nothing_old_keeps_everything() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let db = use_temp_store(dir.path());
    let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap();
    let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
    store.ingest_journal(&journal("recent-1", now_ns)).unwrap();
    drop(store);

    cmd_store_purge(30).unwrap();

    std::env::remove_var("TUMULT_LAKE_PATH");

    let store = tumult_lake::AnalyticsStore::open_read_only(&db).unwrap();
    assert_eq!(store.experiment_count().unwrap(), 1);
}

// ── path ─────────────────────────────────────────────────────

#[test]
fn store_path_reports_missing_store() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    use_temp_store(dir.path());

    cmd_store_path().unwrap();

    std::env::remove_var("TUMULT_LAKE_PATH");
}

#[test]
fn store_path_reports_existing_store() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let db = use_temp_store(dir.path());
    populated_store(&db, &["p-1"]);

    cmd_store_path().unwrap();

    std::env::remove_var("TUMULT_LAKE_PATH");
}

// ── import (happy path) ──────────────────────────────────────

#[test]
fn import_rejects_directory_missing_activities_parquet() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("experiments.parquet"), b"placeholder").unwrap();

    let err = cmd_import(dir.path()).unwrap_err();
    assert!(
        err.to_string().contains("activities.parquet not found"),
        "{err}"
    );
}

#[test]
fn import_loads_backup_into_fresh_store() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();

    // Produce a parquet export from a source store.
    let src_db = dir.path().join("src.duckdb");
    populated_store(&src_db, &["imp-1", "imp-2"]);
    let export_dir = dir.path().join("export");
    std::fs::create_dir_all(&export_dir).unwrap();
    {
        let store = tumult_lake::AnalyticsStore::open(&src_db).unwrap();
        store
            .export_tables(
                &export_dir.join("experiments.parquet"),
                &export_dir.join("activities.parquet"),
            )
            .unwrap();
    }

    // Import it into the (fresh) default store.
    let db = use_temp_store(dir.path());
    cmd_import(&export_dir).unwrap();

    std::env::remove_var("TUMULT_LAKE_PATH");

    let store = tumult_lake::AnalyticsStore::open_read_only(&db).unwrap();
    assert_eq!(store.experiment_count().unwrap(), 2);
}

// ── import-legacy ────────────────────────────────────────────

#[test]
fn store_import_legacy_without_any_source_errors() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    // Isolate every implicit source: no legacy env vars, and a HOME whose
    // ~/.tumult/analytics.duckdb cannot exist.
    std::env::remove_var("TUMULT_ANALYTICS_PATH");
    std::env::remove_var("KRONIKA_DB");
    let original_home = std::env::var_os("HOME");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);

    let err = cmd_store_import_legacy(None, None, None).unwrap_err();
    assert!(err.to_string().contains("no legacy stores found"), "{err}");

    match original_home {
        Some(value) => std::env::set_var("HOME", value),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
fn store_import_legacy_merges_explicit_source() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let legacy = dir.path().join("legacy.duckdb");
    populated_store(&legacy, &["leg-1"]);
    let target = dir.path().join("unified.duckdb");

    cmd_store_import_legacy(Some(&legacy), None, Some(&target)).unwrap();

    let store = tumult_lake::AnalyticsStore::open_read_only(&target).unwrap();
    assert_eq!(store.experiment_count().unwrap(), 1);
}

#[test]
fn store_import_legacy_skips_self_import() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("unified.duckdb");
    populated_store(&db, &["self-1"]);

    // Importing the unified store into itself must be skipped, not merged.
    cmd_store_import_legacy(Some(&db), None, Some(&db)).unwrap();

    let store = tumult_lake::AnalyticsStore::open_read_only(&db).unwrap();
    assert_eq!(store.experiment_count().unwrap(), 1);
}

#[test]
fn store_import_legacy_missing_source_file_errors() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let target = dir.path().join("unified.duckdb");

    let err = cmd_store_import_legacy(
        Some(&dir.path().join("missing.duckdb")),
        None,
        Some(&target),
    )
    .unwrap_err();
    assert!(err.to_string().contains("importing legacy store"), "{err}");
}

// ── migrate ──────────────────────────────────────────────────

// The env guard must cover the awaited migrate call (the env var is read
// inside it). Only tests serialized on ENV_LOCK can block on it, so holding a
// std guard across the await cannot deadlock the test runtime.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn store_migrate_requires_clickhouse_url() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var("TUMULT_CLICKHOUSE_URL");

    let err = cmd_store_migrate().await.unwrap_err();
    assert!(
        err.to_string().contains("TUMULT_CLICKHOUSE_URL not set"),
        "{err}"
    );
}
