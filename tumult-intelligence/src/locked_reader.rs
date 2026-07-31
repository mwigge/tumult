//! A locked-down `DuckDB` reader for executing guard-validated LLM SQL.
//!
//! [`sql_guard`](crate::sql_guard) is the first gate for generated queries;
//! this module is the wall behind it: the connection opens
//! `access_mode = READ_ONLY` **and** `enable_external_access = false`, so
//! even a query that slipped past the guard cannot touch the server file
//! system (`read_text`, `read_csv`, `glob`, …) or the network (`httpfs`).
//! `/api/ask` executes exclusively through this reader.

use std::path::Path;
use std::time::Duration;

use duckdb::types::Value as DuckValue;
use duckdb::{AccessMode, Config, Connection};
use serde_json::{Map as JsonMap, Number, Value as JsonValue};

/// Total attempts an open makes before reporting the store as locked —
/// mirrors `tumult_lake`'s reader open (`open_with_retry`).
const OPEN_ATTEMPTS: u32 = 3;
/// Backoff between open attempts while another process finishes a write.
const OPEN_BACKOFF: Duration = Duration::from_millis(50);

/// Why opening or querying through the locked reader failed.
#[derive(Debug, thiserror::Error)]
pub enum LockedReaderError {
    #[error("could not open the store read-only: {0}")]
    Open(String),
    #[error("query failed: {0}")]
    Query(String),
}

/// Whether a `DuckDB` error is the file-lock conflict raised when another
/// process transiently blocks the open (mirrors `tumult_lake`).
fn is_lock_conflict(err: &duckdb::Error) -> bool {
    matches!(
        err,
        duckdb::Error::DuckDBFailure(_, Some(msg))
            if msg.contains("Could not set lock") || msg.contains("Conflicting lock")
    )
}

/// A read-only `DuckDB` connection with external access disabled.
pub struct LockedReader {
    conn: Connection,
}

impl LockedReader {
    /// Open the store at `path` read-only with `enable_external_access =
    /// false`. The store must already exist and be migrated.
    ///
    /// # Errors
    /// Returns [`LockedReaderError::Open`] if the store cannot be opened
    /// (missing file, persistent lock conflict, or the lock-down settings
    /// are rejected).
    pub fn open(path: &Path) -> Result<Self, LockedReaderError> {
        let mut attempt = 1;
        loop {
            let config = Config::default()
                .access_mode(AccessMode::ReadOnly)
                .and_then(|c| c.enable_external_access(false))
                .map_err(|e| LockedReaderError::Open(e.to_string()))?;
            match Connection::open_with_flags(path, config) {
                Ok(conn) => return Ok(Self { conn }),
                Err(err) if is_lock_conflict(&err) && attempt < OPEN_ATTEMPTS => {
                    std::thread::sleep(OPEN_BACKOFF);
                    attempt += 1;
                }
                Err(err) => return Err(LockedReaderError::Open(err.to_string())),
            }
        }
    }

    /// Run a read-only query and return each row as a JSON object
    /// (`{column: value}`).
    ///
    /// Values are converted from typed `DuckDB` values rather than
    /// `row_to_json` (which `tumult_lake::Reader::query_json_rows` uses):
    /// with external access disabled the JSON extension cannot autoload, so
    /// JSON SQL functions are unavailable on this connection by design.
    ///
    /// # Errors
    /// Returns [`LockedReaderError::Query`] if the query fails to prepare or
    /// execute — including the permission error any external-access attempt
    /// (`read_text`, `read_csv`, …) raises on this connection.
    pub fn query_json_rows(&self, sql: &str) -> Result<Vec<JsonValue>, LockedReaderError> {
        let fail = |e: duckdb::Error| LockedReaderError::Query(e.to_string());
        let mut stmt = self.conn.prepare(sql).map_err(fail)?;
        let mut rows = stmt.query([]).map_err(fail)?;
        // Column names are only available once the statement has executed.
        let names = rows
            .as_ref()
            .map_or_else(Vec::new, duckdb::Statement::column_names);
        let mut out = Vec::new();
        while let Some(row) = rows.next().map_err(fail)? {
            let mut obj = JsonMap::with_capacity(names.len());
            for (i, name) in names.iter().enumerate() {
                let value: DuckValue = row.get(i).map_err(fail)?;
                obj.insert(name.clone(), value_to_json(&value));
            }
            out.push(JsonValue::Object(obj));
        }
        Ok(out)
    }
}

/// `f64` → JSON number; NaN/infinity (unrepresentable) become null.
fn json_f64(f: f64) -> JsonValue {
    Number::from_f64(f).map_or(JsonValue::Null, JsonValue::Number)
}

/// Render a `DuckDB` map key as a JSON object key (maps over the telemetry
/// store are `MAP(VARCHAR, VARCHAR)`).
fn map_key_string(key: &DuckValue) -> String {
    match key {
        DuckValue::Text(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

/// Convert a typed `DuckDB` value to JSON. Scalars keep their JSON types;
/// lists/structs/maps convert recursively; temporal and other exotic types
/// (unused by the telemetry schema) fall back to their debug rendering.
fn value_to_json(value: &DuckValue) -> JsonValue {
    match value {
        DuckValue::Null => JsonValue::Null,
        DuckValue::Boolean(b) => JsonValue::Bool(*b),
        DuckValue::TinyInt(i) => JsonValue::from(*i),
        DuckValue::SmallInt(i) => JsonValue::from(*i),
        DuckValue::Int(i) => JsonValue::from(*i),
        DuckValue::BigInt(i) => JsonValue::from(*i),
        DuckValue::HugeInt(i) => {
            i64::try_from(*i).map_or_else(|_| JsonValue::from(i.to_string()), JsonValue::from)
        }
        DuckValue::UTinyInt(u) => JsonValue::from(*u),
        DuckValue::USmallInt(u) => JsonValue::from(*u),
        DuckValue::UInt(u) => JsonValue::from(*u),
        DuckValue::UBigInt(u) => JsonValue::from(*u),
        DuckValue::Float(f) => json_f64(f64::from(*f)),
        DuckValue::Double(f) => json_f64(*f),
        DuckValue::Decimal(d) => d.to_string().parse::<f64>().map_or_else(
            |_| JsonValue::from(d.to_string()),
            |f| Number::from_f64(f).map_or(JsonValue::Null, JsonValue::Number),
        ),
        DuckValue::Text(s) | DuckValue::Enum(s) => JsonValue::from(s.clone()),
        DuckValue::Blob(b) => JsonValue::from(String::from_utf8_lossy(b).into_owned()),
        DuckValue::List(items) | DuckValue::Array(items) => {
            JsonValue::Array(items.iter().map(value_to_json).collect())
        }
        DuckValue::Struct(fields) => JsonValue::Object(
            fields
                .iter()
                .map(|(k, v)| (k.clone(), value_to_json(v)))
                .collect(),
        ),
        DuckValue::Map(entries) => JsonValue::Object(
            entries
                .iter()
                .map(|(k, v)| (map_key_string(k), value_to_json(v)))
                .collect(),
        ),
        DuckValue::Union(inner) => value_to_json(inner),
        other => JsonValue::from(format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A migrated, then released store in a temp dir.
    fn store_path(dir: &tempfile::TempDir) -> std::path::PathBuf {
        let path = dir.path().join("lake.duckdb");
        drop(tumult_lake::Store::open(&path).unwrap());
        path
    }

    #[test]
    fn opens_with_external_access_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let reader = LockedReader::open(&store_path(&dir)).unwrap();
        let rows = reader
            .query_json_rows("SELECT current_setting('enable_external_access') AS v")
            .unwrap();
        assert_eq!(rows, vec![serde_json::json!({"v": false})]);
    }

    #[test]
    fn file_reading_functions_are_blocked() {
        let dir = tempfile::tempdir().unwrap();
        let reader = LockedReader::open(&store_path(&dir)).unwrap();
        for sql in [
            "SELECT read_text('/proc/self/environ') AS v",
            "SELECT * FROM read_csv('/etc/passwd')",
            "SELECT * FROM glob('/etc/*')",
        ] {
            assert!(
                reader.query_json_rows(sql).is_err(),
                "{sql} must fail with external access disabled"
            );
        }
    }

    #[test]
    fn ordinary_selects_still_work() {
        let dir = tempfile::tempdir().unwrap();
        let reader = LockedReader::open(&store_path(&dir)).unwrap();
        let rows = reader.query_json_rows("SELECT 42 AS n").unwrap();
        assert_eq!(rows, vec![serde_json::json!({"n": 42})]);
    }

    #[test]
    fn connection_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let reader = LockedReader::open(&store_path(&dir)).unwrap();
        assert!(reader.query_json_rows("CREATE TABLE t (a INT)").is_err());
    }
}
