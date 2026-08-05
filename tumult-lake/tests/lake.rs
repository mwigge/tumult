//! Parquet lake export + retention tests (moved out of `src/lake.rs`):
//! incremental/idempotent export, snapshot fingerprints, watermark-guarded
//! retention, and the legacy `_meta.json` fingerprint migration.

#![cfg(feature = "duckdb")]

use tumult_lake::duckdb_store::sample_journal;
use tumult_lake::lake::{
    enforce_retention, export, fingerprint_sql, meta_path, now_ns, status, LakeConfig, AUDIT_TABLE,
    MANUAL_TABLE,
};
use tumult_lake::{LogRow, MetricSumRow, Reader, SpanRow, Store, Writer};

const DAY_NS: i64 = 86_400 * 1_000_000_000;
// Fixed base so test rows land on deterministic dates.
const BASE_NS: i64 = 1_785_225_600_000_000_000; // 2026-07-23T00:00:00Z

fn span(ts_ns: i64, name: &str) -> SpanRow {
    SpanRow {
        ts_ns,
        trace_id: format!("trace-{ts_ns}"),
        span_id: format!("span-{ts_ns}"),
        span_name: name.into(),
        duration_ns: 1_000_000,
        service_name: "tumult".into(),
        ..SpanRow::default()
    }
}

fn fixture() -> (tempfile::TempDir, Store, LakeConfig) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let cfg = LakeConfig::new(d.path().join("lake"), 0);
    (d, store, cfg)
}

fn parquet_count(reader: &Reader, cfg: &LakeConfig, table: &str) -> i64 {
    let glob = cfg.dir.join(format!("{table}/date=*/*.parquet"));
    reader
        .query_json_rows(&format!(
            "SELECT count(*) AS n FROM read_parquet('{}')",
            glob.display()
        ))
        .unwrap()
        .first()
        .and_then(|r| r.get("n"))
        .and_then(serde_json::Value::as_i64)
        .unwrap()
}

#[test]
fn export_creates_valid_parquet_with_matching_row_counts() {
    let (_d, store, cfg) = fixture();
    let writer = store.writer().unwrap();
    writer
        .insert_spans(&[
            span(BASE_NS, "resilience.experiment"),
            span(BASE_NS + 1, "resilience.experiment"),
            span(BASE_NS + DAY_NS, "resilience.action"),
        ])
        .unwrap();
    writer
        .insert_logs(&[LogRow {
            ts_ns: BASE_NS,
            severity_text: "INFO".into(),
            body: "hello".into(),
            ..LogRow::default()
        }])
        .unwrap();
    writer
        .insert_metric_sums(&[MetricSumRow {
            ts_ns: BASE_NS,
            metric_name: "tumult.runs".into(),
            value: 1.0,
            ..MetricSumRow::default()
        }])
        .unwrap();

    let reader = store.read_only().unwrap();
    let report = export(&reader, &cfg).unwrap();

    let spans = report.tables.iter().find(|t| t.name == "spans").unwrap();
    assert_eq!(spans.rows, 3);
    assert_eq!(spans.files.len(), 2, "two day partitions"); // d0 and d1
    assert_eq!(spans.watermark_ns, BASE_NS + DAY_NS);
    // Files exist on disk and read back with the full row count.
    for rel in &spans.files {
        assert!(cfg.dir.join(rel).exists(), "{rel} missing");
    }
    assert_eq!(parquet_count(&reader, &cfg, "spans"), 3);
    assert_eq!(parquet_count(&reader, &cfg, "logs"), 1);
    assert_eq!(parquet_count(&reader, &cfg, "metric_sums"), 1);
}

#[test]
fn export_is_incremental_and_idempotent() {
    let (_d, store, cfg) = fixture();
    let writer = store.writer().unwrap();
    writer.insert_spans(&[span(BASE_NS, "a")]).unwrap();
    let reader = store.read_only().unwrap();

    let first = export(&reader, &cfg).unwrap();
    assert_eq!(first.tables[0].rows, 1);

    // Re-run with no new rows: nothing written, watermark unchanged.
    let second = export(&reader, &cfg).unwrap();
    assert!(second
        .tables
        .iter()
        .all(|t| t.rows == 0 && t.files.is_empty()));
    let files_after_noop = status(&cfg).unwrap().files;

    // New row: exactly one new file, watermark advances, lake total grows.
    // (A read-only connection pins its snapshot at open; a fresh reader
    // per unit of work sees later commits — the scheduler opens one per
    // run for exactly this reason.)
    writer
        .insert_spans(&[span(BASE_NS + 2 * DAY_NS, "b")])
        .unwrap();
    let reader2 = store.read_only().unwrap();
    let third = export(&reader2, &cfg).unwrap();
    let spans = third.tables.iter().find(|t| t.name == "spans").unwrap();
    assert_eq!(spans.rows, 1);
    assert_eq!(spans.watermark_ns, BASE_NS + 2 * DAY_NS);
    assert_eq!(status(&cfg).unwrap().files, files_after_noop + 1);
    assert_eq!(parquet_count(&reader2, &cfg, "spans"), 2);
}

#[test]
fn retention_deletes_only_exported_old_rows() {
    let (_d, store, mut cfg) = fixture();
    cfg.retention_days = 1;
    let writer = store.writer().unwrap();
    // "Old" relative to the test's own clock: 3 days before now.
    let now = now_ns();
    let old = now - 3 * DAY_NS;
    let fresh = now - 1_000_000_000;
    writer
        .insert_spans(&[span(old, "old"), span(fresh, "fresh")])
        .unwrap();

    let reader = store.read_only().unwrap();
    export(&reader, &cfg).unwrap();
    // Lands after the export: above the watermark, so NOT yet exported —
    // the watermark check must protect it even if it were old enough.
    writer.insert_spans(&[span(now, "late")]).unwrap();

    let deleted = enforce_retention(&writer, &cfg).unwrap();
    assert_eq!(deleted.get("spans"), Some(&1));
    // Fresh reader: the one above pinned its snapshot before the delete.
    let reader2 = store.read_only().unwrap();
    let remaining = reader2
        .query_json_rows("SELECT span_name FROM spans ORDER BY ts_ns")
        .unwrap();
    let names: Vec<&str> = remaining
        .iter()
        .filter_map(|r| r.get("span_name").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(names, ["fresh", "late"]);
}

#[test]
fn audit_exports_but_is_never_deleted() {
    let (_d, store, mut cfg) = fixture();
    cfg.retention_days = 1;
    let writer = store.writer().unwrap();
    let old = now_ns() - 3 * DAY_NS;
    writer
        .execute(
            "INSERT INTO manual_experiment_audit VALUES \
             ('a1', 'exp-1', 'alice', ?, 'create', NULL, NULL, 'hash1')",
            [old],
        )
        .unwrap();

    let reader = store.read_only().unwrap();
    let report = export(&reader, &cfg).unwrap();
    let audit = report
        .tables
        .iter()
        .find(|t| t.name == AUDIT_TABLE)
        .unwrap();
    assert_eq!(audit.rows, 1);
    assert_eq!(parquet_count(&reader, &cfg, AUDIT_TABLE), 1);

    let deleted = enforce_retention(&writer, &cfg).unwrap();
    assert!(!deleted.contains_key(AUDIT_TABLE));
    let n = reader
        .query_json_rows("SELECT count(*) AS n FROM manual_experiment_audit")
        .unwrap();
    assert_eq!(n[0]["n"], serde_json::json!(1));
}

fn insert_manual(writer: &Writer, id: &str, hash: &str) {
    writer
        .execute(
            "INSERT INTO manual_experiments (id, experiment_name, exercise_type, \
             executed_at_ns, hypothesis, method, outcome_status, entered_by, \
             entered_at_ns, attestation, content_hash) \
             VALUES (?, 'm', 'drill', 1, 'h', 'm', 'passed', 'alice', 1, 'attest', ?)",
            duckdb::params![id, hash],
        )
        .unwrap();
}

#[test]
fn manual_snapshot_skips_when_unchanged_and_rewrites_on_change() {
    let (_d, store, cfg) = fixture();
    let writer = store.writer().unwrap();
    insert_manual(&writer, "m1", "hash1");
    let reader = store.read_only().unwrap();

    let first = export(&reader, &cfg).unwrap();
    let manual = first
        .tables
        .iter()
        .find(|t| t.name == MANUAL_TABLE)
        .unwrap();
    assert_eq!(manual.rows, 1);
    assert_eq!(manual.files.len(), 1);

    // Unchanged register: no new snapshot file.
    let second = export(&reader, &cfg).unwrap();
    let manual = second
        .tables
        .iter()
        .find(|t| t.name == MANUAL_TABLE)
        .unwrap();
    assert_eq!(manual.rows, 0);
    assert!(manual.files.is_empty());

    // Changed register: new snapshot written.
    insert_manual(&writer, "m2", "hash2");
    let reader2 = store.read_only().unwrap();
    let third = export(&reader2, &cfg).unwrap();
    let manual = third
        .tables
        .iter()
        .find(|t| t.name == MANUAL_TABLE)
        .unwrap();
    assert_eq!(manual.rows, 2);
    assert_eq!(manual.files.len(), 1);
    assert_eq!(parquet_count(&reader2, &cfg, MANUAL_TABLE), 3);
}

fn journal(id: &str, started_at_ns: i64) -> tumult_core::types::Journal {
    let mut j = sample_journal(id, tumult_core::types::ExperimentStatus::Completed);
    j.started_at_ns = started_at_ns;
    j.method_results[0].started_at_ns = started_at_ns + 1;
    j
}

#[test]
fn journal_tables_export_incrementally() {
    let (_d, store, cfg) = fixture();
    let writer = store.writer().unwrap();
    writer
        .ingest_journal(&journal("j1", BASE_NS), None)
        .unwrap();
    let reader = store.read_only().unwrap();

    let first = export(&reader, &cfg).unwrap();
    let exp = first
        .tables
        .iter()
        .find(|t| t.name == "experiments")
        .unwrap();
    assert_eq!(exp.rows, 1);
    assert_eq!(exp.watermark_ns, BASE_NS);
    let acts = first
        .tables
        .iter()
        .find(|t| t.name == "activity_results")
        .unwrap();
    assert_eq!(acts.rows, 1);
    assert_eq!(parquet_count(&reader, &cfg, "experiments"), 1);
    assert_eq!(parquet_count(&reader, &cfg, "activity_results"), 1);

    // Idempotent re-run: nothing new anywhere (journal tables above their
    // watermark, graph snapshots unchanged).
    let second = export(&reader, &cfg).unwrap();
    assert!(second
        .tables
        .iter()
        .all(|t| t.rows == 0 && t.files.is_empty()));

    // A new journal exports only its own rows.
    writer
        .ingest_journal(&journal("j2", BASE_NS + DAY_NS), None)
        .unwrap();
    let reader2 = store.read_only().unwrap();
    let third = export(&reader2, &cfg).unwrap();
    let exp = third
        .tables
        .iter()
        .find(|t| t.name == "experiments")
        .unwrap();
    assert_eq!(exp.rows, 1);
    assert_eq!(exp.watermark_ns, BASE_NS + DAY_NS);
    assert_eq!(parquet_count(&reader2, &cfg, "experiments"), 2);
}

fn insert_decision(writer: &Writer, id: &str, decided_at_ns: i64) {
    writer
        .execute(
            "INSERT INTO autopilot_decisions VALUES \
             (?, ?, 'trigger', 'svc', NULL, 'plug', 'act', 'art', 0.9, \
             '[]', 'high', NULL, '{}', 'ok', '[]', '{}', 'ph', NULL)",
            duckdb::params![id, decided_at_ns],
        )
        .unwrap();
}

#[test]
fn snapshot_tables_skip_unchanged_and_rewrite_on_change() {
    let (_d, store, cfg) = fixture();
    let writer = store.writer().unwrap();
    insert_decision(&writer, "d1", BASE_NS);
    writer
        .execute(
            "INSERT INTO graph_nodes VALUES ('svc:a', 'service', 'a', '{}')",
            [],
        )
        .unwrap();
    let reader = store.read_only().unwrap();
    // The schema seeds graph_nodes (compliance articles, fault domains),
    // so assert against the actual baseline rather than a constant.
    let baseline_nodes = reader
        .query_json_rows("SELECT count(*) AS n FROM graph_nodes")
        .unwrap()[0]["n"]
        .as_u64()
        .unwrap();

    let first = export(&reader, &cfg).unwrap();
    let ad = first
        .tables
        .iter()
        .find(|t| t.name == "autopilot_decisions")
        .unwrap();
    assert_eq!(ad.rows, 1);
    assert_eq!(ad.files.len(), 1);
    let gn = first
        .tables
        .iter()
        .find(|t| t.name == "graph_nodes")
        .unwrap();
    assert_eq!(gn.rows, baseline_nodes);
    assert_eq!(gn.files.len(), 1);

    // Unchanged: both snapshots skipped.
    let second = export(&reader, &cfg).unwrap();
    for name in ["autopilot_decisions", "graph_nodes"] {
        let t = second.tables.iter().find(|t| t.name == name).unwrap();
        assert_eq!(t.rows, 0, "{name}");
        assert!(t.files.is_empty(), "{name}");
    }

    // A new decision rewrites only that table's snapshot.
    insert_decision(&writer, "d2", BASE_NS + 1);
    let reader2 = store.read_only().unwrap();
    let third = export(&reader2, &cfg).unwrap();
    let ad = third
        .tables
        .iter()
        .find(|t| t.name == "autopilot_decisions")
        .unwrap();
    assert_eq!(ad.rows, 2);
    assert_eq!(ad.files.len(), 1);
    let gn = third
        .tables
        .iter()
        .find(|t| t.name == "graph_nodes")
        .unwrap();
    assert_eq!(gn.rows, 0);
    assert_eq!(parquet_count(&reader2, &cfg, "autopilot_decisions"), 3);
}

#[test]
fn autopilot_retention_purges_only_after_fingerprinted_export() {
    let (_d, store, mut cfg) = fixture();
    cfg.retention_days = 1;
    let writer = store.writer().unwrap();
    let old = now_ns() - 3 * DAY_NS;
    insert_decision(&writer, "d1", old);

    let decision_count = |store: &Store| {
        store
            .read_only()
            .unwrap()
            .query_json_rows("SELECT count(*) AS n FROM autopilot_decisions")
            .unwrap()[0]["n"]
            .as_i64()
            .unwrap()
    };

    // Never exported: no fingerprint on record → nothing may be deleted.
    let deleted = enforce_retention(&writer, &cfg).unwrap();
    assert!(!deleted.contains_key("autopilot_decisions"));
    assert_eq!(decision_count(&store), 1);

    let reader = store.read_only().unwrap();
    export(&reader, &cfg).unwrap();

    // d2 lands after the export: the fingerprint no longer covers the
    // hot store, so even old-enough rows survive.
    insert_decision(&writer, "d2", old);
    let deleted = enforce_retention(&writer, &cfg).unwrap();
    assert!(!deleted.contains_key("autopilot_decisions"));
    assert_eq!(decision_count(&store), 2);

    // After a covering export, old rows are purged.
    let reader2 = store.read_only().unwrap();
    export(&reader2, &cfg).unwrap();
    let deleted = enforce_retention(&writer, &cfg).unwrap();
    assert_eq!(deleted.get("autopilot_decisions"), Some(&2));
    assert_eq!(decision_count(&store), 0);
}

#[test]
fn snapshot_only_tables_are_retention_exempt() {
    let (_d, store, mut cfg) = fixture();
    cfg.retention_days = 1;
    let writer = store.writer().unwrap();
    writer
        .execute(
            "INSERT INTO graph_nodes VALUES ('svc:a', 'service', 'a', '{}')",
            [],
        )
        .unwrap();
    writer
        .execute(
            "INSERT INTO agentic_runs VALUES \
             ('r1', 'e1', 'http', 'scenario', 0.0, NULL, NULL)",
            [],
        )
        .unwrap();
    let reader = store.read_only().unwrap();
    export(&reader, &cfg).unwrap();
    // graph_nodes is seeded by the schema; assert it survives intact,
    // whatever the baseline was.
    let baseline_nodes = reader
        .query_json_rows("SELECT count(*) AS n FROM graph_nodes")
        .unwrap()[0]["n"]
        .clone();

    let deleted = enforce_retention(&writer, &cfg).unwrap();
    assert!(!deleted.contains_key("graph_nodes"));
    assert!(!deleted.contains_key("agentic_runs"));
    let n = reader
        .query_json_rows("SELECT count(*) AS n FROM graph_nodes")
        .unwrap();
    assert_eq!(n[0]["n"], baseline_nodes);
    let n = reader
        .query_json_rows("SELECT count(*) AS n FROM agentic_runs")
        .unwrap();
    assert_eq!(n[0]["n"], serde_json::json!(1));
}

#[test]
fn legacy_manual_fingerprint_migrates_into_fingerprints() {
    let (_d, store, cfg) = fixture();
    let writer = store.writer().unwrap();
    insert_manual(&writer, "m1", "hash1");
    let reader = store.read_only().unwrap();
    let fp = reader
        .query_json_rows(&fingerprint_sql(MANUAL_TABLE))
        .unwrap()[0]["fp"]
        .as_str()
        .unwrap()
        .to_string();

    // A pre-generalization meta file carries only the legacy field; the
    // first export under the new code must treat the snapshot as current.
    std::fs::create_dir_all(&cfg.dir).unwrap();
    std::fs::write(
        meta_path(&cfg.dir),
        format!(r#"{{"manual_fingerprint": "{fp}"}}"#),
    )
    .unwrap();
    let report = export(&reader, &cfg).unwrap();
    let manual = report
        .tables
        .iter()
        .find(|t| t.name == MANUAL_TABLE)
        .unwrap();
    assert_eq!(manual.rows, 0);
    assert!(manual.files.is_empty());

    // The rewritten meta carries the generic map, not the legacy field.
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(meta_path(&cfg.dir)).unwrap()).unwrap();
    assert!(raw.get("manual_fingerprint").is_none());
    assert_eq!(raw["fingerprints"][MANUAL_TABLE], serde_json::json!(fp));
}

#[test]
fn export_covers_the_run_system_tables() {
    use tumult_lake::{NewRun, ScheduleRow, WebhookRow};

    let (_d, store, cfg) = fixture();
    let writer = store.writer().unwrap();
    // One run with an audit trail row, one schedule, one webhook + cursor,
    // one dead letter: the run-system rows a restore cannot afford to lose.
    writer
        .insert_run(&NewRun {
            id: "run-1".into(),
            registry_id: "reg-1".into(),
            params_json: None,
            queued_at_ns: BASE_NS,
            actor: Some("tester".into()),
        })
        .unwrap();
    writer
        .create_schedule(&ScheduleRow {
            id: "sched-1".into(),
            name: "nightly".into(),
            registry_id: "reg-1".into(),
            interval_s: 3600,
            vars_json: None,
            env: "dev".into(),
            target: None,
            enabled: true,
            next_run_at_ns: BASE_NS,
            last_run_at_ns: None,
            last_run_id: None,
            created_by: Some("tester".into()),
            created_at_ns: BASE_NS,
        })
        .unwrap();
    writer
        .create_webhook(&WebhookRow {
            id: "w-1".into(),
            name: "hook".into(),
            url: "https://hooks.example.com/x".into(),
            secret: "s".into(),
            events: vec![],
            enabled: true,
            created_by: Some("tester".into()),
            created_at_ns: BASE_NS,
        })
        .unwrap();
    writer.set_webhook_cursor("w-1", BASE_NS).unwrap();
    writer
        .insert_webhook_dead_letter(&tumult_lake::WebhookDeadLetter {
            webhook_id: "w-1".into(),
            run_id: "run-1".into(),
            at_ns: BASE_NS,
            event: "enqueued".into(),
            detail: None,
            actor: Some("tester".into()),
            error: "connection refused".into(),
            attempts: 5,
            dead_at_ns: BASE_NS,
        })
        .unwrap();

    let reader = store.read_only().unwrap();
    let report = export(&reader, &cfg).unwrap();
    for table in [
        "runs",
        "run_registry",
        "run_audit",
        "run_schedules",
        "webhooks",
        "webhook_cursors",
        "webhook_dead_letters",
        "approval_requests",
        "approval_decisions",
        "users",
    ] {
        assert!(
            report.tables.iter().any(|t| t.name == table),
            "{table} missing from the export report"
        );
    }
    assert_eq!(parquet_count(&reader, &cfg, "runs"), 1);
    assert_eq!(parquet_count(&reader, &cfg, "run_audit"), 1);
    assert_eq!(parquet_count(&reader, &cfg, "run_schedules"), 1);
    assert_eq!(parquet_count(&reader, &cfg, "webhooks"), 1);
    assert_eq!(parquet_count(&reader, &cfg, "webhook_cursors"), 1);
    assert_eq!(parquet_count(&reader, &cfg, "webhook_dead_letters"), 1);

    // run_audit is incremental: a new audit row exports on the next run
    // without rewriting what is already in the lake.
    writer
        .insert_run_audit("run-1", "started", None, None)
        .unwrap();
    let reader = store.read_only().unwrap();
    let report = export(&reader, &cfg).unwrap();
    let audit = report
        .tables
        .iter()
        .find(|t| t.name == "run_audit")
        .unwrap();
    assert_eq!(audit.rows, 1, "only the new audit row exports");
    assert_eq!(parquet_count(&reader, &cfg, "run_audit"), 2);
}
