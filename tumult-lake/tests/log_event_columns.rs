//! Schema v21 log columns: `logs.event_name` and `logs.scope_attrs` keep the
//! OTLP `EventName` and the instrumentation-scope attributes of a record.

use tumult_lake::{LogRow, Store, CURRENT_SCHEMA_VERSION};

const TS: i64 = 1_774_980_000_000_000_000;

fn named_log() -> LogRow {
    LogRow {
        ts_ns: TS,
        severity_text: "INFO".into(),
        body: "state".into(),
        service_name: "collector".into(),
        event_name: Some("entity.state".into()),
        scope_attrs: vec![("otel.entity.event_as_log".into(), "true".into())],
        ..LogRow::default()
    }
}

fn log_columns(store: &Store) -> Vec<serde_json::Value> {
    store
        .read_only()
        .unwrap()
        .query_json_rows(
            "SELECT body, event_name, scope_attrs['otel.entity.event_as_log'] AS flag \
             FROM logs ORDER BY body",
        )
        .unwrap()
}

#[test]
fn a_log_row_keeps_its_event_name_and_scope_attributes() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let plain = LogRow {
        ts_ns: TS,
        body: "plain".into(),
        ..LogRow::default()
    };
    store
        .writer()
        .unwrap()
        .insert_logs(&[named_log(), plain])
        .unwrap();

    let rows = log_columns(&store);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["body"], "plain");
    assert_eq!(rows[0]["event_name"], serde_json::Value::Null);
    assert_eq!(rows[0]["flag"], serde_json::Value::Null);
    assert_eq!(rows[1]["event_name"], "entity.state");
    assert_eq!(rows[1]["flag"], "true");
}

/// The `logs` table as schema v20 created it: eight columns and the
/// timestamp index.
const V20_LOGS: [&str; 5] = [
    "DROP TABLE logs",
    "CREATE TABLE logs (
        ts_ns BIGINT NOT NULL,
        severity_text VARCHAR NOT NULL,
        body VARCHAR NOT NULL,
        trace_id VARCHAR,
        span_id VARCHAR,
        service_name VARCHAR NOT NULL,
        log_attrs MAP (VARCHAR, VARCHAR),
        resource_attrs MAP (VARCHAR, VARCHAR)
    )",
    "CREATE INDEX idx_logs_ts_ns ON logs (ts_ns)",
    "INSERT INTO logs VALUES (1, 'INFO', 'old', NULL, NULL, 'svc', MAP {}, MAP {})",
    "UPDATE schema_meta SET value = 20 WHERE key = 'version'",
];

#[test]
fn a_v20_lake_gains_the_log_columns_and_keeps_its_logs() {
    let d = tempfile::TempDir::new().unwrap();
    let path = d.path().join("kronika.duckdb");
    {
        let store = Store::open(&path).unwrap();
        let writer = store.writer().unwrap();
        for sql in V20_LOGS {
            writer.execute(sql, []).unwrap();
        }
        assert_eq!(writer.schema_version().unwrap(), 20);
    }
    // Opening twice proves the migration is idempotent.
    for _ in 0..2 {
        let store = Store::open(&path).unwrap();
        let writer = store.writer().unwrap();
        assert_eq!(writer.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    }
    let store = Store::open(&path).unwrap();
    store.writer().unwrap().insert_logs(&[named_log()]).unwrap();
    let rows = log_columns(&store);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["body"], "old");
    assert_eq!(rows[0]["event_name"], serde_json::Value::Null);
    assert_eq!(rows[1]["event_name"], "entity.state");
    assert_eq!(rows[1]["flag"], "true");
}
