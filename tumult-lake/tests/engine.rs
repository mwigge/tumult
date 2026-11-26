//! The `DuckDB` engine that opens the lake: the store reads its version and
//! the storage format of the lake file, and the workspace pins the engine
//! so that a lockfile update cannot move it.

#![cfg(feature = "duckdb")]

use std::ffi::CStr;
use std::path::Path;

use tumult_lake::Store;
use tumult_otel::logging::testing::capture_json;

/// The version of the `DuckDB` library that this build links, from its C API.
fn linked_engine() -> String {
    // SAFETY: `duckdb_library_version` takes no argument and returns a
    // pointer to a static string that ends in NUL.
    let version = unsafe { CStr::from_ptr(duckdb::ffi::duckdb_library_version()) };
    version.to_str().expect("the version is UTF-8").to_string()
}

/// The storage format that `DuckDB` gives a new file.
fn storage_of_a_new_file(dir: &Path) -> String {
    let conn = duckdb::Connection::open(dir.join("plain.duckdb")).expect("a plain file");
    conn.query_row(
        "SELECT tags['storage_version'] FROM duckdb_databases() \
         WHERE database_name = current_database()",
        [],
        |row| row.get(0),
    )
    .expect("the storage tag")
}

#[test]
fn opening_a_new_lake_records_the_linked_engine_and_the_storage_format_of_the_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let db = dir.path().join("kronika.duckdb");

    let logs = capture_json(tracing::Level::INFO, || {
        Store::open(&db).unwrap();
    });

    let expected_storage = storage_of_a_new_file(dir.path());
    let opened = logs.named("lake.store.opened");
    assert_eq!(opened.len(), 1, "{}", logs.raw);
    assert_eq!(opened[0]["lake.engine"], linked_engine(), "{}", logs.raw);
    assert_eq!(opened[0]["lake.storage"], expected_storage, "{}", logs.raw);
    assert_eq!(
        opened[0]["lake.schema.version"],
        tumult_lake::CURRENT_SCHEMA_VERSION,
        "{}",
        logs.raw
    );
    let versions = Store::at(&db)
        .read_only()
        .unwrap()
        .engine_versions()
        .unwrap();
    assert_eq!(versions.engine, linked_engine());
    assert_eq!(versions.storage, expected_storage);
}

#[test]
fn the_workspace_pins_duckdb_with_a_tilde_so_the_engine_moves_only_in_its_own_change() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../Cargo.toml");
    let manifest = std::fs::read_to_string(manifest).unwrap();
    let line = manifest
        .lines()
        .find(|line| line.starts_with("duckdb = "))
        .expect("the workspace names duckdb");
    let requirement = line
        .split("version = \"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the duckdb line has a version");
    // The second part of a duckdb-rs version names the engine: 1.20000 is
    // DuckDB 2.0.0. A tilde with that part allows only patch releases of
    // the same engine.
    let pinned = requirement
        .strip_prefix('~')
        .is_some_and(|version| version.split('.').count() >= 2);
    assert!(pinned, "duckdb must be pinned with a tilde: {line}");
}
