//! Complete CLI backups respect `DuckDB`'s cross-process writer lock.

use std::process::Command;
use tempfile::TempDir;

#[test]
fn backup_refuses_a_live_writer_and_succeeds_after_shutdown() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("lake.duckdb");
    // A writer holding the exclusive lock stands in for the live daemon.
    let daemon = tumult_lake::AnalyticsStore::open(&db_path).unwrap();
    let out = dir.path().join("backup");
    let backup = || {
        Command::new(env!("CARGO_BIN_EXE_tumult"))
            .env("TUMULT_LAKE_PATH", &db_path)
            .args(["store", "backup", "--output"])
            .arg(&out)
            .output()
            .unwrap()
    };
    let locked = backup();
    assert!(
        !locked.status.success(),
        "a separate writer must prevent backup"
    );
    assert!(!out.exists(), "a refused backup must not publish a bundle");

    drop(daemon);
    let completed = backup();
    assert!(
        completed.status.success(),
        "{}",
        String::from_utf8_lossy(&completed.stderr)
    );

    assert!(out.join("store.duckdb").exists());
    assert!(out.join("manifest.json").exists());
}
