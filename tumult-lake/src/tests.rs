//! Store round-trip and schema-migration tests (moved out of `lib.rs`).

use super::*;
use duckdb::Connection;

fn sample_span(experiment_id: &str) -> SpanRow {
    SpanRow {
        ts_ns: 1_774_980_000_000_000_000,
        trace_id: "abc123".into(),
        span_id: "span-1".into(),
        parent_span_id: None,
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: 300_000_000_000,
        status_code: "Ok".into(),
        status_message: String::new(),
        service_name: "tumult".into(),
        service_version: Some("2.18.0".into()),
        experiment_id: Some(experiment_id.into()),
        experiment_name: Some("pg-failover".into()),
        outcome_status: Some("completed".into()),
        fault_type: Some("termination".into()),
        fault_subtype: Some("process-kill".into()),
        fault_severity: Some("major".into()),
        blast_radius: Some("single-instance".into()),
        target_system: Some("database".into()),
        target_technology: Some("postgresql".into()),
        target_environment: Some("staging".into()),
        plugin_name: Some("tumult-ssh".into()),
        hypothesis_met: Some(true),
        recovery_time_s: Some(12.5),
        span_attrs: vec![(
            "resilience.baseline.probe.query_latency.mean".into(),
            "0.042".into(),
        )],
        resource_attrs: vec![("service.namespace".into(), "chaos".into())],
        events: "[]".into(),
    }
}

#[test]
fn open_creates_schema_and_roundtrips_span() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();

    let writer = store.writer().unwrap();
    assert_eq!(writer.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    writer.insert_spans(&[sample_span("exp-1")]).unwrap();

    let reader = store.read_only().unwrap();
    let runs = reader.experiment_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].experiment_id.as_deref(), Some("exp-1"));
    assert_eq!(runs[0].outcome_status.as_deref(), Some("completed"));
    assert_eq!(runs[0].duration_ns, Some(300_000_000_000));
}

#[test]
fn map_and_json_columns_roundtrip() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    store
        .writer()
        .unwrap()
        .insert_spans(&[sample_span("exp-1")])
        .unwrap();

    let reader = store.read_only().unwrap();
    let rows = reader
        .query_json_rows(
            "SELECT span_attrs['resilience.baseline.probe.query_latency.mean'] AS probe_mean,
                        resource_attrs['service.namespace'] AS ns,
                        fault_type
                 FROM spans",
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["probe_mean"], serde_json::json!("0.042"));
    assert_eq!(rows[0]["ns"], serde_json::json!("chaos"));
    assert_eq!(rows[0]["fault_type"], serde_json::json!("termination"));
}

#[test]
fn histogram_arrays_roundtrip() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    store
        .writer()
        .unwrap()
        .insert_metric_histograms(&[MetricHistogramRow {
            ts_ns: 1,
            metric_name: "tumult.experiment.duration".into(),
            count: 7,
            sum: 42.0,
            min: Some(1.0),
            max: Some(9.0),
            bucket_counts: vec![1, 2, 4],
            explicit_bounds: vec![5.0, 10.0],
            experiment_name: Some("exp".into()),
            outcome_status: Some("success".into()),
            plugin_name: Some("process".into()),
            attrs: vec![],
            resource_attrs: vec![],
        }])
        .unwrap();

    let reader = store.read_only().unwrap();
    let rows = reader
        .query_json_rows("SELECT count, bucket_counts, explicit_bounds FROM metric_histograms")
        .unwrap();
    assert_eq!(rows[0]["count"], serde_json::json!(7));
    assert_eq!(rows[0]["bucket_counts"], serde_json::json!([1, 2, 4]));
    assert_eq!(rows[0]["explicit_bounds"], serde_json::json!([5.0, 10.0]));
    let dims = reader
        .query_json_rows(
            "SELECT experiment_name, outcome_status, plugin_name FROM metric_histograms",
        )
        .unwrap();
    assert_eq!(dims[0]["experiment_name"], serde_json::json!("exp"));
    assert_eq!(dims[0]["plugin_name"], serde_json::json!("process"));
}

#[test]
fn import_batch_is_recorded() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    writer
        .record_import_batch(&ImportBatch {
            id: "batch-1".into(),
            source: "journal.json".into(),
            imported_at_ns: 1,
            rows: 3,
            label: Some("manual".into()),
        })
        .unwrap();
    let reader = store.read_only().unwrap();
    let rows = reader
        .query_json_rows("SELECT source, rows FROM import_batches")
        .unwrap();
    assert_eq!(rows[0]["rows"], serde_json::json!(3));
}

/// `StoreLocked` only triggers CROSS-process: duckdb-rs caches one
/// in-process database instance per path, so a second `writer()` in the
/// same process opens another connection to the same instance instead of
/// hitting the file lock. The ingest daemon still keeps a single Writer
/// by construction (one channel, one task).
#[test]
fn second_writer_in_same_process_shares_the_instance() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let w1 = store.writer().unwrap();
    w1.insert_spans(&[sample_span("exp-1")]).unwrap();
    let w2 = store.writer().unwrap();
    w2.insert_spans(&[sample_span("exp-2")]).unwrap();
    let reader = store.read_only().unwrap();
    assert_eq!(reader.experiment_runs().unwrap().len(), 2);
}

/// `Writer::ingest_journal` writes through the single-writer connection;
/// the same rows are then readable through a read-only `Reader` and an
/// `AnalyticsStore` opened on the same file in-process.
#[test]
fn writer_ingests_journal_visible_to_reader() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();

    let journal =
        duckdb_store::sample_journal("wj-1", tumult_core::types::ExperimentStatus::Completed);
    assert!(writer.ingest_journal(&journal, None).unwrap());
    // Duplicate experiment_id: skipped, not duplicated.
    assert!(!writer.ingest_journal(&journal, None).unwrap());

    let reader = store.read_only().unwrap();
    let rows = reader
        .query_json_rows("SELECT experiment_id, status FROM experiments")
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["experiment_id"], serde_json::json!("wj-1"));
    let activities = reader
        .query_json_rows("SELECT count(*) AS n FROM activity_results")
        .unwrap();
    assert_eq!(activities[0]["n"], serde_json::json!(1));
}

#[test]
fn experiment_runs_resolves_outcome_from_completed_log() {
    // tumult leaves span.outcome_status NULL; the outcome lives on the
    // `experiment.completed` log record's capitalised `status` attr.
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let mut span = sample_span("exp-log");
    span.outcome_status = None;
    writer.insert_spans(&[span]).unwrap();
    writer
        .insert_logs(&[LogRow {
            ts_ns: 1_774_980_300_000_000_000,
            severity_text: "INFO".into(),
            body: "experiment.completed".into(),
            trace_id: Some("abc123".into()),
            span_id: None,
            service_name: "tumult".into(),
            log_attrs: vec![
                ("experiment_id".into(), "exp-log".into()),
                ("status".into(), "Deviated".into()),
            ],
            resource_attrs: vec![],
        }])
        .unwrap();
    let reader = store.read_only().unwrap();
    let runs = reader.experiment_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome_status.as_deref(), Some("Deviated"));
    // The span's own outcome still wins when present.
    let writer = store.writer().unwrap();
    writer.insert_spans(&[sample_span("exp-span")]).unwrap();
    // Fresh reader: read-only connections pin their snapshot at open.
    let reader2 = store.read_only().unwrap();
    let rows = reader2
        .query_json_rows(
            "SELECT experiment_id, outcome_status FROM experiment_runs ORDER BY experiment_id",
        )
        .unwrap();
    assert_eq!(rows[1]["outcome_status"], serde_json::json!("completed"));
}

#[test]
fn read_only_reader_coexists_with_open_writer() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    writer.insert_spans(&[sample_span("exp-1")]).unwrap();

    let reader = store.read_only().unwrap();
    assert_eq!(reader.experiment_runs().unwrap().len(), 1);
    drop(writer);
}

/// Schema v3: a fresh open creates the unified analytics family
/// (journal detail, agentic, autopilot, ChaosGraph) alongside the
/// telemetry tables, at the current version, with the static
/// compliance-article nodes seeded.
#[test]
fn v3_open_creates_unified_analytics_family() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("lake.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    assert_eq!(writer.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);

    let reader = store.read_only().unwrap();
    for table in [
        "experiments",
        "activity_results",
        "load_results",
        "agentic_runs",
        "agentic_contract_outcomes",
        "agentic_fault_applications",
        "agentic_replay_outcomes",
        "autopilot_decisions",
        "autopilot_events",
        "autopilot_change_events",
        "graph_nodes",
        "graph_edges",
        "run_registry",
        "runs",
        "run_audit",
    ] {
        let rows = reader
            .query_json_rows(&format!(
                "SELECT count(*) AS c FROM information_schema.tables \
                     WHERE table_name = '{table}'"
            ))
            .unwrap();
        assert_eq!(rows[0]["c"], serde_json::json!(1), "missing {table}");
    }

    let articles = reader
        .query_json_rows("SELECT count(*) AS c FROM graph_nodes WHERE kind = 'compliance_article'")
        .unwrap();
    assert_eq!(
        articles[0]["c"],
        serde_json::json!(tumult_graph::compliance_article_nodes().len() as u64)
    );
    // The v3 edges attrs column exists.
    reader
        .query_json_rows("SELECT attrs FROM graph_edges LIMIT 0")
        .unwrap();
}

/// A v2-shaped store (telemetry + manual tables only, version 2) gains
/// the analytics family on open, keeps its data, and advances to v3.
#[test]
fn v2_store_migrates_forward_without_data_loss() {
    let d = tempfile::TempDir::new().unwrap();
    let db_path = d.path().join("lake.duckdb");

    // Seed a v2-shaped store: current v2 DDL minus the v3 family, one
    // span row, version recorded as 2.
    {
        let conn = Connection::open(&db_path).unwrap();
        let v2_ddl = schema::CREATE_TABLES
            .split("-- v3: the tumult-analytics family")
            .next()
            .unwrap();
        conn.execute_batch(v2_ddl).unwrap();
        conn.execute(
            "INSERT INTO schema_meta (key, value) VALUES ('version', 2)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO spans VALUES (
                    1, 't', 's', NULL, 'resilience.experiment', 'Internal', 1,
                    'Ok', '', 'tumult', NULL, 'legacy-exp', 'legacy', 'completed',
                    NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
                    CAST('{}' AS MAP(VARCHAR,VARCHAR)),
                    CAST('{}' AS MAP(VARCHAR,VARCHAR)), '[]')",
            [],
        )
        .unwrap();
    }

    let store = Store::open(&db_path).unwrap();
    let writer = store.writer().unwrap();
    assert_eq!(writer.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);
    let reader = store.read_only().unwrap();
    // Prior data preserved; analytics family now queryable.
    assert_eq!(reader.experiment_runs().unwrap().len(), 1);
    reader
        .query_json_rows("SELECT count(*) AS c FROM experiments")
        .unwrap();
    let articles = reader
        .query_json_rows("SELECT count(*) AS c FROM graph_nodes WHERE kind = 'compliance_article'")
        .unwrap();
    assert!(articles[0]["c"].as_u64().unwrap() > 0);
}
