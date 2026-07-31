//! Cross-process concurrency tests for the `DuckDB` single-writer model.
//!
//! `DuckDB` caches the database instance per path *within a process*, so a
//! second same-process open of the same file shares the instance and never
//! conflicts. The real CLI-vs-MCP-server collision — and the `StoreLocked`
//! error that reports it — is only observable across processes. These tests
//! therefore re-exec this test binary as a child process (via
//! [`std::env::current_exe`]) while the parent holds the store open.

#![cfg(feature = "duckdb")]

use std::path::Path;
use std::process::Command;

use tumult_lake::{AnalyticsError, AnalyticsStore};

/// Env var carrying the child's mode: `write` or `read`.
const CHILD_MODE: &str = "TUMULT_STORE_CONCURRENCY_MODE";
/// Env var carrying the store path the child should open.
const CHILD_PATH: &str = "TUMULT_STORE_CONCURRENCY_PATH";
/// Prefix the child prints its outcome under, for the parent to parse.
const MARK: &str = "CONCURRENCY_CHILD::";

/// Child entrypoint. During a normal `cargo test` run [`CHILD_MODE`] is unset,
/// so this returns immediately as a no-op. When the parent re-execs this binary
/// with the env vars set, it performs the requested open and prints the outcome.
#[test]
fn concurrency_child_worker() {
    let Ok(mode) = std::env::var(CHILD_MODE) else {
        return;
    };
    let path = std::env::var(CHILD_PATH).expect("child path env must be set");
    let path = Path::new(&path);

    match mode.as_str() {
        "write" => match AnalyticsStore::open(path) {
            Ok(_) => println!("{MARK}WRITE_OK"),
            Err(AnalyticsError::StoreLocked { .. }) => println!("{MARK}WRITE_STORE_LOCKED"),
            Err(e) => println!("{MARK}WRITE_OTHER_ERR::{e}"),
        },
        "read" => match AnalyticsStore::open_read_only(path) {
            Ok(store) => {
                let count = store.experiment_count().expect("read-only query works");
                println!("{MARK}READ_OK::{count}");
            }
            Err(e) => println!("{MARK}READ_ERR::{e}"),
        },
        other => panic!("unknown child mode: {other}"),
    }
}

/// Spawn a child process re-running [`concurrency_child_worker`] in `mode`
/// against `path`, and return the outcome token it printed.
fn run_child(mode: &str, path: &Path) -> String {
    let exe = std::env::current_exe().expect("current test exe");
    let output = Command::new(exe)
        .args(["--exact", "concurrency_child_worker", "--nocapture"])
        .env(CHILD_MODE, mode)
        .env(CHILD_PATH, path)
        .output()
        .expect("spawn child test process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| line.strip_prefix(MARK))
        .unwrap_or_else(|| panic!("child printed no {MARK} marker; stdout:\n{stdout}"))
        .to_string()
}

/// A second read-WRITE open from another process, while the first writer holds
/// the store, fails with the clear typed `StoreLocked` error — not an opaque
/// `DuckDB` lock message.
#[test]
fn second_writer_gets_store_locked() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("analytics.duckdb");

    // Parent holds the exclusive write lock for the duration of the child run.
    let writer = AnalyticsStore::open(&db_path).unwrap();

    let outcome = run_child("write", &db_path);
    assert_eq!(
        outcome, "WRITE_STORE_LOCKED",
        "a second cross-process writer must report StoreLocked"
    );

    drop(writer);
}

/// Two read-ONLY opens from different processes coexist: while the parent holds
/// the store open read-only, a child also opens it read-only and can query it.
#[test]
fn two_readers_coexist_across_processes() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("analytics.duckdb");

    // Initialise the schema with a writer, then release the write lock.
    drop(AnalyticsStore::open(&db_path).unwrap());

    // Parent holds a read-only handle while the child opens read-only too.
    let reader = AnalyticsStore::open_read_only(&db_path).unwrap();
    let outcome = run_child("read", &db_path);
    assert_eq!(
        outcome, "READ_OK::0",
        "a second cross-process reader must open and query successfully"
    );

    // Parent's read-only handle is still usable.
    assert_eq!(reader.experiment_count().unwrap(), 0);
}
