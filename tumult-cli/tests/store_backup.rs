//! Regression: `tumult store backup` opens the store read-only, so it
//! works against a live daemon (or any process) holding the write lock —
//! previously it failed with `StoreLocked`.

use tempfile::TempDir;

#[test]
fn backup_works_while_another_process_holds_the_write_lock() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("lake.duckdb");
    // A writer holding the exclusive lock stands in for the live daemon.
    let _daemon = tumult_lake::AnalyticsStore::open(&db_path).unwrap();
    std::env::set_var("TUMULT_LAKE_PATH", &db_path);

    let out = dir.path().join("backup");
    tumult_cli::commands::cmd_store_backup(&out)
        .expect("backup must succeed against a locked (live) store");

    assert!(out.join("experiments.parquet").exists());
    assert!(out.join("activities.parquet").exists());
    std::env::remove_var("TUMULT_LAKE_PATH");
}
