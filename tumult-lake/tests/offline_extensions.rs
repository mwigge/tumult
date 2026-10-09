//! Release builds must work without extension downloads or a user's cache.
#![cfg(feature = "duckdb")]

use duckdb::{Config, Connection};

fn offline_connection(directory: &std::path::Path) -> Connection {
    let config = Config::default()
        .enable_autoload_extension(false)
        .unwrap()
        .with("extension_directory", directory.to_str().unwrap())
        .unwrap();
    Connection::open_in_memory_with_flags(config).unwrap()
}

fn assert_statically_linked(connection: &Connection, extension: &str) {
    let (loaded, mode): (bool, String) = connection
        .query_row(
            "SELECT loaded, install_mode FROM duckdb_extensions() WHERE extension_name = ?",
            [extension],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(
        loaded,
        "{extension} must already be loaded without autoload"
    );
    assert_eq!(
        mode, "STATICALLY_LINKED",
        "{extension} must ship in the binary"
    );
}

#[test]
fn offline_json_schema_and_queries_need_no_downloads() {
    let directory = tempfile::tempdir().unwrap();
    let extensions = directory.path().join("empty-extension-cache");
    let connection = offline_connection(&extensions);
    connection
        .execute_batch("CREATE TABLE evidence (document JSON); INSERT INTO evidence VALUES ('{\"status\":\"ready\"}');")
        .expect("JSON schema creation must work in a clean offline installation");
    let result: String = connection
        .query_row(
            "SELECT json_extract_string(document, '$.status') FROM evidence",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(result, "ready");
    let encoded: String = connection
        .query_row(
            "SELECT row_to_json(t) FROM (SELECT 42 AS answer) t",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(encoded, r#"{"answer":42}"#);
    assert_statically_linked(&connection, "json");
    assert!(
        !extensions.exists(),
        "no extension installation may be attempted"
    );
}

#[test]
fn offline_parquet_export_and_read_need_no_downloads() {
    let directory = tempfile::tempdir().unwrap();
    let extensions = directory.path().join("empty-extension-cache");
    let connection = offline_connection(&extensions);
    let parquet = directory.path().join("evidence.parquet");
    let quoted = parquet.to_string_lossy().replace('\'', "''");
    connection
        .execute_batch(&format!(
            "COPY (SELECT 42::BIGINT AS answer) TO '{quoted}' (FORMAT PARQUET)"
        ))
        .expect("Parquet export must work in a clean offline installation");
    let result: i64 = connection
        .query_row(
            &format!("SELECT answer FROM read_parquet('{quoted}')"),
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(result, 42);
    assert_statically_linked(&connection, "parquet");
    assert!(
        !extensions.exists(),
        "no extension installation may be attempted"
    );
}
