//! Parquet lake export + retention tests (moved out of `src/lake.rs`):
//! manifest snapshots, idempotent retries, delayed arrivals, private data
//! exclusions, explicit migration and fail-closed retention.

#![cfg(feature = "duckdb")]

use tumult_lake::duckdb_store::sample_journal;
use tumult_lake::lake::{
    enforce_retention, export, meta_path, now_ns, status, LakeConfig, AUDIT_TABLE, MANUAL_TABLE,
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
    let glob = tumult_lake::lake::committed_file(cfg, table).unwrap();
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
    assert_eq!(spans.files.len(), 1, "one complete table snapshot");
    assert!(spans.watermark_ns > 0);
    // Files exist on disk and read back with the full row count.
    for rel in &spans.files {
        assert!(cfg.dir.join(rel).exists(), "{rel} missing");
    }
    assert_eq!(parquet_count(&reader, &cfg, "spans"), 3);
    assert_eq!(parquet_count(&reader, &cfg, "logs"), 1);
    assert_eq!(parquet_count(&reader, &cfg, "metric_sums"), 1);
}

#[test]
fn export_snapshots_are_idempotent() {
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
    assert_eq!(spans.rows, 2);
    assert!(spans.watermark_ns > 0);
    assert_eq!(status(&cfg).unwrap().files, files_after_noop);
    assert_eq!(parquet_count(&reader2, &cfg, "spans"), 2);
}

#[test]
fn retention_keeps_all_history_until_cold_queries_exist() {
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

    assert!(enforce_retention(&writer, &cfg).is_err());
    // Fresh reader: the one above pinned its snapshot before the delete.
    let reader2 = store.read_only().unwrap();
    let remaining = reader2
        .query_json_rows("SELECT span_name FROM spans ORDER BY ts_ns")
        .unwrap();
    let names: Vec<&str> = remaining
        .iter()
        .filter_map(|r| r.get("span_name").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(names, ["old", "fresh", "late"]);
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

    assert!(enforce_retention(&writer, &cfg).is_err());
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
    assert_eq!(parquet_count(&reader2, &cfg, MANUAL_TABLE), 2);
}

fn journal(id: &str, started_at_ns: i64) -> tumult_core::types::Journal {
    let mut j = sample_journal(id, tumult_core::types::ExperimentStatus::Completed);
    j.started_at_ns = started_at_ns;
    j.method_results[0].started_at_ns = started_at_ns + 1;
    j
}

#[test]
fn journal_tables_export_complete_snapshots() {
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
    assert!(exp.watermark_ns > 0);
    let acts = first
        .tables
        .iter()
        .find(|t| t.name == "activity_results")
        .unwrap();
    assert_eq!(acts.rows, 1);
    assert_eq!(parquet_count(&reader, &cfg, "experiments"), 1);
    assert_eq!(parquet_count(&reader, &cfg, "activity_results"), 1);

    // Idempotent re-run: all committed table snapshots are unchanged.
    let second = export(&reader, &cfg).unwrap();
    assert!(second
        .tables
        .iter()
        .all(|t| t.rows == 0 && t.files.is_empty()));

    // A new journal publishes a complete updated table snapshot.
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
    assert_eq!(exp.rows, 2);
    assert!(exp.watermark_ns > 0);
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
    assert_eq!(parquet_count(&reader2, &cfg, "autopilot_decisions"), 2);
}

#[test]
fn autopilot_history_survives_retention_requests() {
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

    // An archive cannot justify deletion until cold queries are available.
    assert!(enforce_retention(&writer, &cfg).is_err());
    assert_eq!(decision_count(&store), 1);

    let reader = store.read_only().unwrap();
    export(&reader, &cfg).unwrap();

    // d2 lands after the export: the fingerprint no longer covers the
    // hot store, so even old-enough rows survive.
    insert_decision(&writer, "d2", old);
    assert!(enforce_retention(&writer, &cfg).is_err());
    assert_eq!(decision_count(&store), 2);

    // Even after a covering export, historical reports still require hot rows.
    let reader2 = store.read_only().unwrap();
    export(&reader2, &cfg).unwrap();
    assert!(enforce_retention(&writer, &cfg).is_err());
    assert_eq!(decision_count(&store), 2);
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

    assert!(enforce_retention(&writer, &cfg).is_err());
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
fn legacy_archive_is_preserved_and_requires_explicit_migration() {
    let (_dir, store, cfg) = fixture();
    std::fs::create_dir_all(&cfg.dir).unwrap();
    let raw = r#"{"last_export_ns":1,"tables":{"spans":100},"manual_fingerprint":"legacy"}"#;
    std::fs::write(meta_path(&cfg.dir), raw).unwrap();
    let error = export(&store.read_only().unwrap(), &cfg).unwrap_err();
    assert!(error.to_string().contains("legacy lake archive"));
    assert_eq!(std::fs::read_to_string(meta_path(&cfg.dir)).unwrap(), raw);
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
        "webhook_cursors",
        "webhook_dead_letters",
        "approval_requests",
        "approval_decisions",
    ] {
        assert!(
            report.tables.iter().any(|t| t.name == table),
            "{table} missing from the export report"
        );
    }
    assert_eq!(parquet_count(&reader, &cfg, "runs"), 1);
    assert_eq!(parquet_count(&reader, &cfg, "run_audit"), 1);
    assert_eq!(parquet_count(&reader, &cfg, "run_schedules"), 1);
    assert!(tumult_lake::lake::committed_file(&cfg, "webhooks").is_err());
    assert_eq!(parquet_count(&reader, &cfg, "webhook_cursors"), 1);
    assert_eq!(parquet_count(&reader, &cfg, "webhook_dead_letters"), 1);

    // A new audit row publishes a complete version without appending
    // duplicate logical rows to the committed snapshot.
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
    assert_eq!(audit.rows, 2, "complete audit snapshot exports");
    assert_eq!(parquet_count(&reader, &cfg, "run_audit"), 2);
}

#[test]
fn late_and_tied_events_are_exported_and_retention_fails_closed() {
    let (_dir, store, mut cfg) = fixture();
    let writer = store.writer().unwrap();
    writer.insert_spans(&[span(BASE_NS, "first")]).unwrap();
    export(&store.read_only().unwrap(), &cfg).unwrap();
    writer
        .insert_spans(&[span(BASE_NS - DAY_NS, "late"), span(BASE_NS, "tied")])
        .unwrap();
    let report = export(&store.read_only().unwrap(), &cfg).unwrap();
    assert_eq!(
        report
            .tables
            .iter()
            .find(|t| t.name == "spans")
            .unwrap()
            .rows,
        3,
        "complete snapshot includes late and tied records"
    );
    cfg.retention_days = 1;
    assert!(
        enforce_retention(&writer, &cfg).is_err(),
        "historical reports must retain their source rows until cold querying exists"
    );
    assert_eq!(
        store
            .read_only()
            .unwrap()
            .query_json_rows("SELECT count(*) AS n FROM spans")
            .unwrap()[0]["n"],
        3
    );
}

#[test]
fn portable_export_includes_provenance_and_excludes_credentials() {
    let (_dir, store, cfg) = fixture();
    let report = export(&store.read_only().unwrap(), &cfg).unwrap();
    let names: Vec<_> = report.tables.iter().map(|t| t.name.as_str()).collect();
    for required in [
        "runs",
        "run_registry",
        "run_audit",
        "approval_requests",
        "approval_decisions",
        "evidence_attachments",
        "import_batches",
    ] {
        assert!(names.contains(&required), "missing {required}");
    }
    for secret in ["users", "tokens", "sessions", "webhooks"] {
        assert!(!names.contains(&secret), "portable export exposes {secret}");
    }
}

#[test]
fn failed_publication_retries_without_duplicate_logical_rows() {
    let (_dir, store, cfg) = fixture();
    let writer = store.writer().unwrap();
    writer.insert_spans(&[span(BASE_NS, "one")]).unwrap();
    export(&store.read_only().unwrap(), &cfg).unwrap();
    let old_manifest = std::fs::read(meta_path(&cfg.dir)).unwrap();
    writer.insert_spans(&[span(BASE_NS + 1, "two")]).unwrap();
    // Fail metadata publication after all new data files have been written.
    let blocker = cfg.dir.join("_meta.json.tmp");
    std::fs::create_dir(&blocker).unwrap();
    assert!(export(&store.read_only().unwrap(), &cfg).is_err());
    assert_eq!(std::fs::read(meta_path(&cfg.dir)).unwrap(), old_manifest);
    assert_eq!(parquet_count(&store.read_only().unwrap(), &cfg, "spans"), 1);
    std::fs::remove_dir(blocker).unwrap();
    export(&store.read_only().unwrap(), &cfg).unwrap();
    assert_eq!(parquet_count(&store.read_only().unwrap(), &cfg, "spans"), 2);
    let again = export(&store.read_only().unwrap(), &cfg).unwrap();
    assert!(again.tables.iter().all(|t| t.files.is_empty()));
}

#[test]
fn empty_table_snapshot_replaces_previously_nonempty_version() {
    let (_dir, store, cfg) = fixture();
    let writer = store.writer().unwrap();
    writer.insert_spans(&[span(BASE_NS, "one")]).unwrap();
    export(&store.read_only().unwrap(), &cfg).unwrap();
    writer.execute("DELETE FROM spans", []).unwrap();
    export(&store.read_only().unwrap(), &cfg).unwrap();
    assert_eq!(parquet_count(&store.read_only().unwrap(), &cfg, "spans"), 0);
}

#[cfg(unix)]
#[test]
fn metadata_temporary_symlink_cannot_overwrite_another_file() {
    let (dir, store, cfg) = fixture();
    export(&store.read_only().unwrap(), &cfg).unwrap();
    let unrelated = dir.path().join("unrelated.txt");
    std::fs::write(&unrelated, "keep me").unwrap();
    std::os::unix::fs::symlink(&unrelated, cfg.dir.join("_meta.json.tmp")).unwrap();
    export(&store.read_only().unwrap(), &cfg).unwrap();
    assert_eq!(std::fs::read_to_string(unrelated).unwrap(), "keep me");
}

#[test]
fn empty_snapshot_preserves_schema_changes() {
    let (_d, store, cfg) = fixture();
    let reader = store.read_only().unwrap();
    export(&reader, &cfg).unwrap();
    let before = tumult_lake::lake::committed_file(&cfg, "metric_gauges").unwrap();
    store
        .writer()
        .unwrap()
        .execute(
            "ALTER TABLE metric_gauges ADD COLUMN future_field VARCHAR",
            [],
        )
        .unwrap();
    let reader = store.read_only().unwrap();
    export(&reader, &cfg).unwrap();
    let after = tumult_lake::lake::committed_file(&cfg, "metric_gauges").unwrap();
    assert_ne!(
        before, after,
        "schema is part of a snapshot's identity, even without rows"
    );
    reader
        .query_json_rows(&format!(
            "SELECT future_field FROM read_parquet('{}')",
            after.display()
        ))
        .unwrap();
}

#[test]
fn corrupted_snapshot_is_rejected_and_rebuilt_from_retained_rows() {
    let (_dir, store, cfg) = fixture();
    store
        .writer()
        .unwrap()
        .insert_spans(&[span(BASE_NS, "one")])
        .unwrap();
    export(&store.read_only().unwrap(), &cfg).unwrap();
    let path = tumult_lake::lake::committed_file(&cfg, "spans").unwrap();
    std::fs::write(&path, b"truncated parquet").unwrap();
    assert!(tumult_lake::lake::committed_file(&cfg, "spans").is_err());
    export(&store.read_only().unwrap(), &cfg).unwrap();
    assert_eq!(parquet_count(&store.read_only().unwrap(), &cfg, "spans"), 1);
}
