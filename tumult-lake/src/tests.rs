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

/// The log record a run leaves when it ends, with `status` as the run
/// spelled it.
fn completed_log(experiment_id: &str, status: &str) -> LogRow {
    LogRow {
        ts_ns: 1_774_980_300_000_000_000,
        severity_text: "INFO".into(),
        body: "experiment.completed".into(),
        trace_id: Some("abc123".into()),
        span_id: None,
        service_name: "tumult".into(),
        log_attrs: vec![
            ("experiment_id".into(), experiment_id.into()),
            ("status".into(), status.into()),
        ],
        resource_attrs: vec![],
        ..LogRow::default()
    }
}

#[test]
fn the_outcome_rule_gives_one_spelling_and_reads_the_span_first() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    // One run has its outcome on its log record only, the other on both.
    let mut on_log = sample_span("exp-log");
    on_log.outcome_status = None;
    let mut on_span = sample_span("exp-span");
    on_span.outcome_status = Some("Halted".into());
    writer.insert_spans(&[on_log, on_span]).unwrap();
    writer
        .insert_logs(&[
            completed_log("exp-log", "Deviated"),
            completed_log("exp-span", "Completed"),
        ])
        .unwrap();

    let rows = store
        .read_only()
        .unwrap()
        .query_json_rows(&format!(
            "SELECT s.experiment_id, {outcome} AS outcome \
             FROM spans s LEFT JOIN logs l \
               ON l.log_attrs['experiment_id'] = s.experiment_id \
              AND l.body = 'experiment.completed' \
             ORDER BY s.experiment_id",
            outcome = crate::run_outcome_sql("s", "l")
        ))
        .unwrap();

    let outcomes: Vec<(&str, &str)> = rows
        .iter()
        .map(|row| {
            (
                row["experiment_id"].as_str().unwrap_or_default(),
                row["outcome"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(outcomes, [("exp-log", "deviated"), ("exp-span", "halted")]);
}

#[test]
fn experiment_runs_resolves_outcome_from_completed_log() {
    // A run stored before 3.0.2 has no outcome on its root span; it is on
    // the `experiment.completed` log record, spelled with a capital. The
    // view gives it in the spelling of the journal.
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let mut span = sample_span("exp-log");
    span.outcome_status = None;
    writer.insert_spans(&[span]).unwrap();
    writer
        .insert_logs(&[completed_log("exp-log", "Deviated")])
        .unwrap();
    let reader = store.read_only().unwrap();
    let runs = reader.experiment_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome_status.as_deref(), Some("deviated"));
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

/// Every status a run can end with, read from the spellings a journal may
/// hold, so that a status added to `ExperimentStatus` is in the list.
fn every_status() -> Vec<tumult_core::types::ExperimentStatus> {
    let refused = serde_json::from_str::<tumult_core::types::ExperimentStatus>("\"none\"")
        .expect_err("no run ends as none")
        .to_string();
    let names = refused
        .split_once("expected one of ")
        .and_then(|(_, rest)| rest.split_once(" at line"))
        .map(|(names, _)| names)
        .unwrap_or_else(|| panic!("the refusal does not list the statuses: {refused}"));
    names
        .split(", ")
        .map(|name| serde_json::from_value(serde_json::json!(name.trim_matches('`'))).unwrap())
        .collect()
}

/// The spelling of a run stored before 3.0.2: the status with a capital.
fn with_capital(status: &str) -> String {
    let mut letters = status.chars();
    letters.next().map_or_else(String::new, |first| {
        first.to_ascii_uppercase().to_string() + letters.as_str()
    })
}

#[test]
fn every_status_a_run_ends_with_has_one_verdict() {
    use tumult_core::types::ExperimentStatus as Status;
    // The rule. A status added to the journal does not compile here until
    // it has a verdict.
    let rule = |status: &Status| match status {
        Status::Completed => RunVerdict::Pass,
        Status::Deviated | Status::Failed | Status::Halted => RunVerdict::Fail,
        Status::Aborted | Status::Interrupted => RunVerdict::NoEvidence,
    };
    let statuses = every_status();
    assert_eq!(statuses.len(), 6, "{statuses:?}");

    for status in &statuses {
        let name = status.to_string();
        assert_eq!(run_verdict(&name), Some(rule(status)), "{name}");
        let old = with_capital(&name);
        assert_eq!(run_verdict(&old), Some(rule(status)), "{old}");
    }
    for no_status in ["", "incomplete", "passed"] {
        assert_eq!(run_verdict(no_status), None, "{no_status:?}");
    }
}

#[test]
fn the_verdicts_the_web_is_checked_against_are_the_verdicts_of_the_lake() {
    // The web reads this file in `web/src/lib/tone.test.ts`. A status added
    // to the journal fails here until the file lists it, and the file then
    // fails the web test until the web gives it a tone.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/run-verdicts.json");
    let listed: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();

    let verdicts: std::collections::BTreeMap<String, String> = every_status()
        .iter()
        .map(|status| {
            let verdict = run_verdict(&status.to_string()).map_or("none", RunVerdict::as_str);
            (status.to_string(), verdict.to_string())
        })
        .collect();
    assert_eq!(
        listed, verdicts,
        "tests/run-verdicts.json is not the table of run_verdict"
    );
}

#[test]
fn the_verdict_in_sql_is_the_verdict_in_rust() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let reader = store.read_only().unwrap();
    let mut spellings: Vec<String> = every_status()
        .iter()
        .flat_map(|status| [status.to_string(), with_capital(&status.to_string())])
        .collect();
    spellings.extend([String::new(), "incomplete".to_string()]);

    for spelling in &spellings {
        let rows = reader
            .query_json_rows(&format!(
                "SELECT {} AS verdict",
                run_verdict_sql(&format!("'{spelling}'"))
            ))
            .unwrap();
        assert_eq!(
            rows[0]["verdict"].as_str(),
            run_verdict(spelling).map(RunVerdict::as_str),
            "{spelling:?}"
        );
    }
    let rows = reader
        .query_json_rows(&format!("SELECT {} AS verdict", run_verdict_sql("NULL")))
        .unwrap();
    assert!(
        rows[0]["verdict"].is_null(),
        "a run with no status has no verdict"
    );
    // A reader filters on a verdict by its spelling, so each has its own.
    let spelled: std::collections::BTreeSet<&str> =
        [RunVerdict::Pass, RunVerdict::Fail, RunVerdict::NoEvidence]
            .map(RunVerdict::as_str)
            .into_iter()
            .collect();
    assert_eq!(spelled.len(), 3, "{spelled:?}");
}

#[test]
fn a_run_with_two_completion_logs_is_one_run_that_ended_as_the_later_says() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let mut span = sample_span("exp-twice");
    span.outcome_status = None;
    writer.insert_spans(&[span]).unwrap();
    let earlier = completed_log("exp-twice", "Deviated");
    let later = LogRow {
        ts_ns: earlier.ts_ns + 1,
        ..completed_log("exp-twice", "Completed")
    };
    writer.insert_logs(&[earlier, later]).unwrap();
    let reader = store.read_only().unwrap();

    let rows = reader
        .query_json_rows(&format!(
            "SELECT s.experiment_id, {outcome} AS outcome FROM spans AS s \
             LEFT JOIN run_completions AS l ON l.experiment_id = s.experiment_id \
             WHERE s.span_name = 'resilience.experiment'",
            outcome = crate::run_outcome_sql("s", "l")
        ))
        .unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["outcome"], "completed");
    let runs = reader.experiment_runs().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].outcome_status.as_deref(), Some("completed"));
}

/// A run stored only as a journal, whose journal measured a recovery time
/// of `rto_s`, or none.
fn journal_measuring(id: &str, rto_s: Option<f64>) -> tumult_core::types::Journal {
    let mut journal =
        crate::duckdb_store::sample_journal(id, tumult_core::types::ExperimentStatus::Completed);
    journal.objectives = rto_s.map(|rto| tumult_core::types::ObjectiveOutcome {
        rto_measured_s: Some(rto),
        ..Default::default()
    });
    journal
}

/// The mean recovery time of the runs in `store`, by the rule of the lake.
fn mean_recovery_time(store: &Store) -> serde_json::Value {
    let rows = store
        .read_only()
        .unwrap()
        .query_json_rows(&format!(
            "SELECT {} AS v FROM run_roots AS s",
            crate::mean_recovery_time_sql("s")
        ))
        .unwrap();
    rows[0]["v"].clone()
}

#[test]
fn the_mean_recovery_time_is_of_the_runs_that_report_one_traced_or_not() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    // A traced run that recovered in 20 s, and a span inside its trace that
    // carries a recovery time of its own.
    let root = SpanRow {
        recovery_time_s: Some(20.0),
        ..sample_span("traced")
    };
    let inner = SpanRow {
        span_id: "span-2".into(),
        parent_span_id: Some("span-1".into()),
        span_name: "resilience.action".into(),
        experiment_id: None,
        recovery_time_s: Some(80.0),
        ..sample_span("traced")
    };
    writer.insert_spans(&[root, inner]).unwrap();
    // Two runs stored only as journals: one measured 40 s, one nothing.
    for (id, rto_s) in [("journal-40", Some(40.0)), ("journal-none", None)] {
        assert!(writer
            .ingest_journal(&journal_measuring(id, rto_s), None)
            .unwrap());
    }

    assert_eq!(mean_recovery_time(&store), serde_json::json!(30.0));
}

#[test]
fn with_no_run_that_reports_a_recovery_time_the_mean_is_no_value_and_not_zero() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    assert!(mean_recovery_time(&store).is_null(), "no run at all");

    let writer = store.writer().unwrap();
    let root = SpanRow {
        recovery_time_s: None,
        ..sample_span("traced")
    };
    writer.insert_spans(&[root]).unwrap();
    assert!(writer
        .ingest_journal(&journal_measuring("journal-none", None), None)
        .unwrap());

    assert!(mean_recovery_time(&store).is_null(), "no run reports one");
}

/// A reader that asks about the runs since a time reads the completion
/// logs since then, and still one for each run.
#[test]
fn the_completion_logs_since_a_time_are_the_later_rows_of_the_view() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    let old = completed_log("exp-old", "Completed");
    let at = |ts_ns: i64, status: &str| LogRow {
        ts_ns,
        ..completed_log("exp-new", status)
    };
    let since = old.ts_ns + 10;
    let logs = [
        at(since, "Deviated"),
        at(since + 5, "Completed"),
        at(since - 1, "Failed"),
        old,
    ];
    writer.insert_logs(&logs).unwrap();
    let reader = store.read_only().unwrap();
    let rows = |sql: String| {
        reader
            .query_json_rows(&format!(
                "SELECT experiment_id, ts_ns, log_attrs['status'] AS status \
                 FROM ({sql}) ORDER BY experiment_id"
            ))
            .unwrap()
    };

    let all = rows(crate::run_completions_sql(None));
    assert_eq!(all, rows("SELECT * FROM run_completions".into()));
    assert_eq!(all.len(), 2, "{all:?}");
    let later = rows(crate::run_completions_sql(Some(since)));
    assert_eq!(later.len(), 1, "{later:?}");
    assert_eq!(later[0]["experiment_id"], "exp-new");
    assert_eq!(later[0]["status"], "Completed");
    // The record at the bound itself is read.
    let at_the_bound = rows(crate::run_completions_sql(Some(since + 5)));
    assert_eq!(at_the_bound, later);
    assert_eq!(rows(crate::run_completions_sql(Some(since + 6))).len(), 0);
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
        let v2_ddl = schema::CREATE_TABLES[..2].concat();
        conn.execute_batch(&v2_ddl).unwrap();
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

/// A journal of a deviated run of an experiment that declares its target.
fn journal_with_target(id: &str) -> (tumult_core::types::Journal, tumult_core::types::Experiment) {
    use tumult_core::types::{Experiment, ExperimentStatus, ExperimentTarget, HypothesisResult};
    let mut journal = crate::duckdb_store::sample_journal(id, ExperimentStatus::Deviated);
    journal.steady_state_after = Some(HypothesisResult {
        title: "the target answers".into(),
        met: false,
        probe_results: vec![],
    });
    let experiment = Experiment {
        title: journal.experiment_title.clone(),
        target: Some(ExperimentTarget {
            system: "payments-api".into(),
            technology: Some("postgresql".into()),
            environment: Some("staging".into()),
            criticality: None,
            namespace: None,
            version: None,
        }),
        ..Experiment::default()
    };
    (journal, experiment)
}

fn run_roots(store: &Store) -> Vec<serde_json::Value> {
    store
        .read_only()
        .unwrap()
        .query_json_rows(
            "SELECT experiment_id, experiment_name, span_name, ts_ns, duration_ns, \
             trace_id, outcome_status, status_code, hypothesis_met, target_system, \
             target_technology, target_environment, journal_only \
             FROM run_roots ORDER BY experiment_id",
        )
        .unwrap()
}

#[test]
fn a_run_stored_only_as_a_journal_has_a_root_built_from_the_journal() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let (journal, experiment) = journal_with_target("cli-run");
    let writer = store.writer().unwrap();
    assert!(writer.ingest_journal(&journal, Some(&experiment)).unwrap());

    let roots = run_roots(&store);

    assert_eq!(roots.len(), 1, "{roots:?}");
    let root = &roots[0];
    assert_eq!(root["journal_only"], serde_json::json!(true));
    assert_eq!(root["experiment_id"], "cli-run");
    assert_eq!(root["experiment_name"], "Test cli-run");
    assert_eq!(root["span_name"], "resilience.experiment");
    assert_eq!(root["trace_id"], serde_json::Value::Null);
    assert_eq!(root["ts_ns"], serde_json::json!(journal.started_at_ns));
    assert_eq!(root["duration_ns"], serde_json::json!(300_000_000_000_i64));
    assert_eq!(root["outcome_status"], "deviated");
    assert_eq!(root["status_code"], "Error");
    assert_eq!(root["hypothesis_met"], serde_json::json!(false));
    assert_eq!(root["target_system"], "payments-api");
    assert_eq!(root["target_technology"], "postgresql");
    assert_eq!(root["target_environment"], "staging");
}

#[test]
fn a_journal_stored_without_its_definition_has_a_root_without_a_target() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let journal = crate::duckdb_store::sample_journal(
        "bare",
        tumult_core::types::ExperimentStatus::Completed,
    );
    let writer = store.writer().unwrap();
    assert!(writer.ingest_journal(&journal, None).unwrap());

    let roots = run_roots(&store);

    assert_eq!(roots.len(), 1, "{roots:?}");
    assert_eq!(roots[0]["journal_only"], serde_json::json!(true));
    assert_eq!(roots[0]["outcome_status"], "completed");
    assert_eq!(roots[0]["status_code"], "Ok");
    assert_eq!(roots[0]["hypothesis_met"], serde_json::Value::Null);
    assert_eq!(roots[0]["target_system"], serde_json::Value::Null);
    assert_eq!(roots[0]["target_environment"], serde_json::Value::Null);
}

#[test]
fn a_journal_whose_hypothesis_failed_before_the_method_says_not_met() {
    use tumult_core::types::{ExperimentStatus, HypothesisResult};
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let verdict = |met| HypothesisResult {
        title: "the target answers".into(),
        met,
        probe_results: vec![],
    };
    let mut aborted = crate::duckdb_store::sample_journal("aborted", ExperimentStatus::Aborted);
    aborted.steady_state_before = Some(verdict(false));
    let mut cut_short =
        crate::duckdb_store::sample_journal("cut-short", ExperimentStatus::Interrupted);
    cut_short.steady_state_before = Some(verdict(true));
    let writer = store.writer().unwrap();
    assert!(writer.ingest_journal(&aborted, None).unwrap());
    assert!(writer.ingest_journal(&cut_short, None).unwrap());

    let roots = run_roots(&store);

    assert_eq!(roots[0]["experiment_id"], "aborted");
    assert_eq!(roots[0]["hypothesis_met"], serde_json::json!(false));
    assert_eq!(roots[1]["experiment_id"], "cut-short");
    assert_eq!(roots[1]["hypothesis_met"], serde_json::Value::Null);
}

#[test]
fn the_hypothesis_verdict_of_a_journal_is_one_rule_in_the_view_and_in_the_function() {
    use tumult_core::types::{ExperimentStatus, HypothesisResult};
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let verdict = |met| HypothesisResult {
        title: "the target answers".into(),
        met,
        probe_results: vec![],
    };
    // A step failed while the target held after the method; the target
    // failed before the method; the definition has no hypothesis.
    let mut held = crate::duckdb_store::sample_journal("a-held", ExperimentStatus::Failed);
    held.steady_state_before = Some(verdict(true));
    held.steady_state_after = Some(verdict(true));
    let mut down = crate::duckdb_store::sample_journal("b-down", ExperimentStatus::Aborted);
    down.steady_state_before = Some(verdict(false));
    let unjudged = crate::duckdb_store::sample_journal("c-none", ExperimentStatus::Completed);
    let journals = [held, down, unjudged];
    let writer = store.writer().unwrap();
    for journal in &journals {
        assert!(writer.ingest_journal(journal, None).unwrap());
    }

    let by_the_rule = store
        .read_only()
        .unwrap()
        .query_json_rows(&format!(
            "SELECT {} AS hypothesis_met FROM experiments AS e ORDER BY e.experiment_id",
            crate::hypothesis_met_sql("e")
        ))
        .unwrap();
    let roots = run_roots(&store);

    assert_eq!(roots.len(), journals.len(), "{roots:?}");
    assert_eq!(by_the_rule.len(), journals.len(), "{by_the_rule:?}");
    for ((journal, root), rule) in journals.iter().zip(&roots).zip(&by_the_rule) {
        let met = serde_json::json!(journal.hypothesis_met());
        assert_eq!(root["experiment_id"], journal.experiment_id.as_str());
        assert_eq!(root["hypothesis_met"], met, "{}", journal.experiment_id);
        assert_eq!(rule["hypothesis_met"], met, "{}", journal.experiment_id);
    }
}

#[test]
fn a_run_with_a_root_span_is_one_root_and_the_span_is_what_it_says() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let (journal, experiment) = journal_with_target("daemon-run");
    let writer = store.writer().unwrap();
    writer.insert_spans(&[sample_span("daemon-run")]).unwrap();
    assert!(writer.ingest_journal(&journal, Some(&experiment)).unwrap());

    let roots = run_roots(&store);

    assert_eq!(roots.len(), 1, "{roots:?}");
    assert_eq!(roots[0]["journal_only"], serde_json::json!(false));
    assert_eq!(roots[0]["trace_id"], "abc123");
    assert_eq!(roots[0]["experiment_name"], "pg-failover");
    assert_eq!(roots[0]["outcome_status"], "completed");
    assert_eq!(roots[0]["target_system"], "database");
}

#[test]
fn the_runs_view_lists_a_run_stored_only_as_a_journal() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let (journal, experiment) = journal_with_target("cli-run");
    let writer = store.writer().unwrap();
    writer.insert_spans(&[sample_span("daemon-run")]).unwrap();
    assert!(writer.ingest_journal(&journal, Some(&experiment)).unwrap());

    let runs = store.read_only().unwrap().experiment_runs().unwrap();

    assert_eq!(runs.len(), 2);
    let cli = runs
        .iter()
        .find(|run| run.experiment_id.as_deref() == Some("cli-run"))
        .expect("the journal run is listed");
    assert_eq!(cli.experiment_name.as_deref(), Some("Test cli-run"));
    assert_eq!(cli.outcome_status.as_deref(), Some("deviated"));
    assert_eq!(cli.hypothesis_met, Some(false));
    assert_eq!(cli.started_at_ns, Some(journal.started_at_ns));
    assert_eq!(cli.ended_at_ns, Some(journal.ended_at_ns));
}

#[test]
fn the_run_roots_have_every_column_of_a_span() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let _writer = store.writer().unwrap();

    let missing = store
        .read_only()
        .unwrap()
        .query_json_rows(
            "SELECT column_name FROM information_schema.columns WHERE table_name = 'spans' \
             EXCEPT \
             SELECT column_name FROM information_schema.columns WHERE table_name = 'run_roots'",
        )
        .unwrap();

    assert!(missing.is_empty(), "{missing:?}");
}

/// A statement for each SQL function of the schema that no view uses, as
/// a reader would write it.
fn reader_statements() -> [String; 4] {
    let since = run_completions_sql(Some(1_774_980_000_000_000_000));
    let verdict = run_verdict_sql(&run_outcome_sql("s", "l"));
    let mean = mean_recovery_time_sql("s");
    let register = crate::schema::dr_register_sql(
        "list_value('backup', 'drill', 'failover', 'restore')",
        "1774980000000000000",
    );
    [
        format!("-- run_completions_sql with a window, as a reader joins it.\n{since};"),
        format!(
            "-- run_verdict_sql of the outcome of each run.
SELECT
    s.experiment_id,
    {verdict} AS verdict
FROM run_roots AS s
LEFT JOIN run_completions AS l ON s.experiment_id = l.experiment_id;"
        ),
        format!(
            "-- mean_recovery_time_sql over the roots of the runs.
SELECT {mean} AS mean_recovery_time_s
FROM run_roots AS s;"
        ),
        format!("-- dr_register_sql with the default DR types, as of a time.\n{register};"),
    ]
}

/// The statements a reader writes with the functions of the schema run on
/// a lake and answer for the run in it.
#[test]
fn the_statements_a_reader_writes_with_the_schema_functions_run_on_a_lake() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    writer.insert_spans(&[sample_span("run-1")]).unwrap();
    writer
        .insert_logs(&[completed_log("run-1", "completed")])
        .unwrap();
    let reader = store.read_only().unwrap();

    // A reader runs a statement as a subquery, so without the semicolon
    // that ends it in the file.
    let [since, verdict, mean, register] =
        reader_statements().map(|sql| reader.query_json_rows(sql.trim_end_matches(';')).unwrap());

    assert_eq!(since.len(), 1, "{since:?}");
    assert_eq!(since[0]["experiment_id"], "run-1");
    assert_eq!(
        verdict,
        [serde_json::json!({"experiment_id": "run-1", "verdict": "pass"})]
    );
    assert_eq!(mean, [serde_json::json!({"mean_recovery_time_s": 12.5})]);
    assert_eq!(
        register,
        Vec::<serde_json::Value>::new(),
        "no DR test, no target"
    );
}
