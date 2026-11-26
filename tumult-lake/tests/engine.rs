//! The `DuckDB` engine that opens the lake: the store reads its version and
//! the storage format of the lake file.

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
