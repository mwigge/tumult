//! `import_legacy`: merging pre-unification databases (old tumult-analytics
//! store, kronika lake) into the unified store — column-intersection
//! robustness against older schemas, and idempotency on re-run.

use std::path::Path;

use duckdb::Connection;
use tumult_lake::AnalyticsStore;

/// A legacy tumult-analytics store: v1-shaped `experiments` (12 columns) and
/// an `autopilot_decisions` missing the later `autonomy_score` column.
fn build_legacy_analytics(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE experiments (
            experiment_id VARCHAR NOT NULL, title VARCHAR NOT NULL,
            status VARCHAR NOT NULL, started_at_ns BIGINT NOT NULL,
            ended_at_ns BIGINT NOT NULL, duration_ms UBIGINT NOT NULL,
            method_step_count BIGINT NOT NULL, rollback_count BIGINT NOT NULL,
            hypothesis_before_met BOOLEAN, hypothesis_after_met BOOLEAN,
            estimate_accuracy DOUBLE, resilience_score DOUBLE
        );
        INSERT INTO experiments VALUES
            ('legacy-exp-1', 'Legacy', 'completed', 10, 20, 5, 2, 0,
             NULL, NULL, NULL, 0.9);
        CREATE TABLE autopilot_decisions (
            id VARCHAR PRIMARY KEY, decided_at_ns BIGINT NOT NULL,
            trigger VARCHAR NOT NULL, service_id VARCHAR NOT NULL,
            tier VARCHAR, plugin VARCHAR NOT NULL, action VARCHAR NOT NULL,
            article_id VARCHAR NOT NULL, score DOUBLE NOT NULL,
            reasons JSON NOT NULL, confidence VARCHAR NOT NULL,
            playbook VARCHAR, validator JSON NOT NULL, verdict VARCHAR NOT NULL,
            gate_rules JSON NOT NULL, gate_detail JSON NOT NULL,
            policy_hash VARCHAR NOT NULL
        );
        INSERT INTO autopilot_decisions VALUES
            ('legacy-dec-1', 1_000, 'staleness', 'svc:db', 'data',
             'tumult-postgres', 'kill-connections', 'compliance:DORA/Art.25',
             1.5, '[\"r1\"]', 'high', NULL, '{}', 'propose', '[]', '{}', 'abc');
        CREATE TABLE graph_nodes (
            id VARCHAR PRIMARY KEY, kind VARCHAR NOT NULL, label VARCHAR NOT NULL
        );
        INSERT INTO graph_nodes VALUES ('svc:legacy', 'service', 'legacy');",
    )
    .unwrap();
}

/// A legacy kronika lake: telemetry tables, with `metric_histograms` in its
/// v1 shape — WITHOUT the promoted `experiment_name` / `outcome_status` /
/// `plugin_name` dimension columns.
fn build_legacy_kronika(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE logs (
            ts_ns BIGINT NOT NULL, severity_text VARCHAR NOT NULL,
            body VARCHAR NOT NULL, trace_id VARCHAR, span_id VARCHAR,
            service_name VARCHAR NOT NULL,
            log_attrs MAP(VARCHAR, VARCHAR) NOT NULL,
            resource_attrs MAP(VARCHAR, VARCHAR) NOT NULL
        );
        INSERT INTO logs VALUES
            (42, 'INFO', 'legacy line', 'trace-1', 'span-1', 'kronika',
             CAST('{}' AS MAP(VARCHAR,VARCHAR)),
             CAST('{}' AS MAP(VARCHAR,VARCHAR)));
        CREATE TABLE metric_histograms (
            ts_ns BIGINT NOT NULL, metric_name VARCHAR NOT NULL,
            count UBIGINT NOT NULL, sum DOUBLE NOT NULL,
            min DOUBLE, max DOUBLE,
            bucket_counts BIGINT[] NOT NULL, explicit_bounds DOUBLE[] NOT NULL,
            attrs MAP(VARCHAR, VARCHAR) NOT NULL,
            resource_attrs MAP(VARCHAR, VARCHAR) NOT NULL
        );
        INSERT INTO metric_histograms VALUES
            (42, 'kronika.latency', 3, 9.0, 1.0, 5.0, [1, 2], [2.5],
             CAST('{}' AS MAP(VARCHAR,VARCHAR)),
             CAST('{}' AS MAP(VARCHAR,VARCHAR)));
        CREATE TABLE import_batches (
            id VARCHAR PRIMARY KEY, source VARCHAR NOT NULL,
            imported_at_ns BIGINT NOT NULL, rows BIGINT NOT NULL, label VARCHAR
        );
        INSERT INTO import_batches VALUES ('batch-1', 'legacy', 42, 2, NULL);",
    )
    .unwrap();
}

fn inserted(report: &[(String, usize)], table: &str) -> usize {
    report
        .iter()
        .find(|(t, _)| t == table)
        .map_or(0, |(_, n)| *n)
}

#[test]
fn import_merges_both_legacy_stores_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let analytics_db = dir.path().join("analytics.duckdb");
    let kronika_db = dir.path().join("kronika.duckdb");
    build_legacy_analytics(&analytics_db);
    build_legacy_kronika(&kronika_db);

    let store = AnalyticsStore::open(&dir.path().join("lake.duckdb")).unwrap();

    // The analytics source: experiments, autopilot_decisions (without the
    // autonomy_score column), graph_nodes merge in.
    let report = store.import_legacy(&analytics_db).unwrap();
    assert_eq!(inserted(&report, "experiments"), 1);
    assert_eq!(inserted(&report, "autopilot_decisions"), 1);
    assert_eq!(inserted(&report, "graph_nodes"), 1);
    // Tables absent in the source are not reported.
    assert!(!report.iter().any(|(t, _)| t == "spans"));

    // The kronika source: telemetry tables merge in; the v1 histograms shape
    // imports despite missing the promoted columns.
    let report = store.import_legacy(&kronika_db).unwrap();
    assert_eq!(inserted(&report, "logs"), 1);
    assert_eq!(inserted(&report, "metric_histograms"), 1);
    assert_eq!(inserted(&report, "import_batches"), 1);

    // Content landed and is queryable through the store.
    assert_eq!(store.experiment_count().unwrap(), 1);
    let rows = store
        .query("SELECT service_id, verdict FROM autopilot_decisions")
        .unwrap();
    assert_eq!(rows[0][0], "svc:db");
    let rows = store
        .query("SELECT count(*) FROM graph_nodes WHERE id = 'svc:legacy'")
        .unwrap();
    assert_eq!(rows[0][0], "1");
    let rows = store
        .query("SELECT count(*) FROM metric_histograms WHERE metric_name = 'kronika.latency'")
        .unwrap();
    assert_eq!(rows[0][0], "1");

    // Re-running both imports inserts nothing — natural-key dedupe.
    for (table, n) in store.import_legacy(&analytics_db).unwrap() {
        assert_eq!(n, 0, "re-import must skip existing {table} rows");
    }
    for (table, n) in store.import_legacy(&kronika_db).unwrap() {
        assert_eq!(n, 0, "re-import must skip existing {table} rows");
    }
}

#[test]
fn missing_source_is_a_clean_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = AnalyticsStore::open(&dir.path().join("lake.duckdb")).unwrap();
    let err = store
        .import_legacy(&dir.path().join("nope.duckdb"))
        .unwrap_err();
    assert!(err.to_string().contains("legacy store not found"));
}
