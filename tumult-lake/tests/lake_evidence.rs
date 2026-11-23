//! The Parquet archive holds the evidence a verdict is computed from.
//!
//! A verdict on an objective is only as good as the samples behind it. The
//! archive keeps the probe samples, the objective verdicts and the
//! performance verdicts, and a store filled from the archive holds the same
//! rows. Every table of the schema is either archived or named as left out,
//! so a new table cannot ship without that decision.

#![cfg(feature = "duckdb")]

use std::collections::BTreeSet;

use tumult_core::types::{
    ExperimentStatus, NfrCriticality, NfrKind, NfrOperator, NfrState, NfrVerdict, ObjectiveOutcome,
    ProbeSample, SamplePhase,
};
use tumult_lake::duckdb_store::sample_journal;
use tumult_lake::lake::{committed_file, export, meta_path, LakeConfig};
use tumult_lake::{PerfVerdictRow, Reader, Store};

/// The tables that hold the evidence of a verdict.
const EVIDENCE: [&str; 3] = ["probe_samples", "nfr_verdicts", "perf_verdicts"];

fn fixture() -> (tempfile::TempDir, Store, LakeConfig) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open(&dir.path().join("kronika.duckdb")).unwrap();
    let cfg = LakeConfig::new(dir.path().join("lake"), 0);
    (dir, store, cfg)
}

fn sample(phase: SamplePhase, ts_ns: i64, duration_ms: u64, ok: bool) -> ProbeSample {
    ProbeSample {
        probe: "health".into(),
        phase,
        ts_ns,
        duration_ms,
        ok,
    }
}

/// A run with two probe samples and a verdict on its recovery objective.
fn journal_with_a_verdict() -> tumult_core::types::Journal {
    let mut journal = sample_journal("dr-1", ExperimentStatus::Completed);
    journal.probe_samples = vec![
        sample(SamplePhase::During, 1_000, 120, true),
        sample(SamplePhase::Post, 2_000, 900, false),
    ];
    journal.objectives = Some(ObjectiveOutcome {
        rto_target_s: Some(900.0),
        rpo_target_s: None,
        rto_measured_s: Some(42.5),
        rpo_measured_s: None,
        verdicts: vec![NfrVerdict {
            nfr_id: "rto".into(),
            kind: NfrKind::Rto,
            metric: "recovery.mttr_s".into(),
            operator: NfrOperator::Lte,
            threshold: 900.0,
            observed: Some(42.5),
            state: NfrState::Met,
            margin: Some(857.5),
            criticality: NfrCriticality::Blocking,
            recovered: None,
            reason: None,
        }],
    });
    journal
}

fn perf_verdict() -> PerfVerdictRow {
    PerfVerdictRow {
        target: "checkout".into(),
        probe: "load.latency_p95_ms".into(),
        baseline_version: "1.4.0".into(),
        candidate_version: "1.5.0".into(),
        quantile: "p95".into(),
        baseline_n: 120,
        candidate_n: 118,
        baseline_p50: Some(40.0),
        baseline_p95: Some(180.0),
        baseline_p99: Some(260.0),
        candidate_p50: Some(42.0),
        candidate_p95: Some(230.0),
        candidate_p99: Some(300.0),
        delta: Some(50.0),
        delta_pct: Some(27.8),
        ci_low: Some(31.0),
        ci_high: Some(70.0),
        confidence: 0.95,
        margin: 9.0,
        verdict: "regressed".into(),
        reasons: vec!["p95 difference interval lies above the margin".into()],
        computed_at_ns: 3_000,
        envs: Some(vec!["prod".into()]),
    }
}

/// Every row of `table`, as JSON, in a stable order.
fn rows(reader: &Reader, table: &str) -> Vec<String> {
    let mut rows: Vec<String> = reader
        .query_json_rows(&format!("SELECT * FROM {table}"))
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    rows.sort();
    rows
}

fn names(reader: &Reader, sql: &str) -> BTreeSet<String> {
    reader
        .query_json_rows(sql)
        .unwrap()
        .iter()
        .filter_map(|row| row["name"].as_str().map(str::to_owned))
        .collect()
}

#[test]
fn a_store_filled_from_the_archive_holds_the_evidence_of_a_verdict() {
    let (_dir, store, cfg) = fixture();
    let writer = store.writer().unwrap();
    writer
        .ingest_journal(&journal_with_a_verdict(), None)
        .unwrap();
    writer.insert_perf_verdict(&perf_verdict()).unwrap();
    let reader = store.read_only().unwrap();
    let before: Vec<Vec<String>> = EVIDENCE.iter().map(|table| rows(&reader, table)).collect();
    assert_eq!(
        before.iter().map(Vec::len).collect::<Vec<_>>(),
        [2, 1, 1],
        "the fixture has samples, an objective verdict and a performance verdict"
    );

    export(&reader, &cfg).unwrap();

    let (_other_dir, other, _) = fixture();
    let restore = other.writer().unwrap();
    for table in EVIDENCE {
        let file = committed_file(&cfg, table).unwrap();
        restore
            .execute(
                &format!(
                    "INSERT INTO {table} BY NAME SELECT * FROM read_parquet('{}')",
                    file.display()
                ),
                [],
            )
            .unwrap();
    }
    let restored = other.read_only().unwrap();
    let after: Vec<Vec<String>> = EVIDENCE
        .iter()
        .map(|table| rows(&restored, table))
        .collect();
    assert_eq!(after, before);
}

#[test]
fn every_table_of_the_schema_is_archived_or_named_as_left_out() {
    let (_dir, store, cfg) = fixture();
    let reader = store.read_only().unwrap();
    let report = export(&reader, &cfg).unwrap();

    let tables = names(
        &reader,
        "SELECT table_name AS name FROM information_schema.tables \
         WHERE table_schema = 'main' AND table_type = 'BASE TABLE'",
    );
    let archived: BTreeSet<String> = report.tables.into_iter().map(|t| t.name).collect();
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(meta_path(&cfg.dir)).unwrap()).unwrap();
    let left_out: BTreeSet<String> = meta["excluded_tables"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect();

    let decided: BTreeSet<String> = archived.union(&left_out).cloned().collect();
    assert_eq!(
        tables.difference(&decided).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "tables that are neither archived nor named as left out"
    );
    assert_eq!(
        decided.difference(&tables).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "names in the archive contract that are not a table"
    );
    assert_eq!(
        archived.intersection(&left_out).collect::<Vec<_>>(),
        Vec::<&String>::new(),
        "tables that are both archived and left out"
    );
}
