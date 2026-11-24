//! Schema v23: a probe sample keeps the number its probe read, and the run
//! keeps the RPO it measured. A lake that v21 or v22 left opens with the
//! column added and its samples kept.

#![cfg(feature = "duckdb")]

use serde_json::Value;
use tumult_core::types::{
    ExperimentStatus, Journal, NfrCriticality, NfrKind, NfrOperator, NfrState, NfrVerdict,
    ObjectiveOutcome, ProbeSample, SamplePhase,
};
use tumult_lake::duckdb_store::sample_journal;
use tumult_lake::{Store, CURRENT_SCHEMA_VERSION};

fn sample(probe: &str, phase: SamplePhase, ts_ns: i64, reading: Option<f64>) -> ProbeSample {
    ProbeSample {
        probe: probe.into(),
        phase,
        ts_ns,
        duration_ms: 12,
        ok: true,
        reading,
    }
}

/// A DR run whose probe `data-lost` read 2 s of data lost after the
/// recovery, against an RPO of 1 s.
fn read_back(id: &str) -> Journal {
    let mut journal = sample_journal(id, ExperimentStatus::Deviated);
    journal.probe_samples = vec![
        sample("data-lost", SamplePhase::During, 1_000, Some(0.0)),
        sample("health", SamplePhase::During, 1_001, None),
        sample("data-lost", SamplePhase::Post, 2_000, Some(2.0)),
        sample("health", SamplePhase::Post, 2_001, None),
    ];
    journal.objectives = Some(ObjectiveOutcome {
        rto_target_s: None,
        rpo_target_s: Some(1.0),
        rto_measured_s: None,
        rpo_measured_s: Some(2.0),
        verdicts: vec![NfrVerdict {
            nfr_id: "rpo".into(),
            kind: NfrKind::Rpo,
            metric: "probe.data-lost.lost_s".into(),
            operator: NfrOperator::Lte,
            threshold: 1.0,
            observed: Some(2.0),
            state: NfrState::Breached,
            margin: Some(-1.0),
            criticality: NfrCriticality::Blocking,
            recovered: None,
            reason: None,
        }],
    });
    journal
}

fn rows(store: &Store, sql: &str) -> Vec<Value> {
    store.read_only().unwrap().query_json_rows(sql).unwrap()
}

fn readings(store: &Store, experiment_id: &str) -> Vec<(String, Value)> {
    store
        .read_only()
        .unwrap()
        .query_json_rows_bound(
            "SELECT probe_name, reading FROM probe_samples \
             WHERE experiment_id = ? ORDER BY ts_ns",
            &[experiment_id],
        )
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["probe_name"].as_str().unwrap().to_string(),
                row["reading"].clone(),
            )
        })
        .collect()
}

fn columns_of_probe_samples(store: &Store) -> Vec<String> {
    rows(
        store,
        "SELECT column_name FROM information_schema.columns \
         WHERE table_name = 'probe_samples' ORDER BY ordinal_position",
    )
    .iter()
    .map(|row| row["column_name"].as_str().unwrap().to_string())
    .collect()
}

#[test]
fn a_sample_keeps_the_number_its_probe_read_and_the_run_its_rpo() {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();

    store
        .writer()
        .unwrap()
        .ingest_journal(&read_back("rpo-1"), None)
        .unwrap();

    assert_eq!(
        readings(&store, "rpo-1"),
        [
            ("data-lost".to_string(), Value::from(0.0)),
            ("health".to_string(), Value::Null),
            ("data-lost".to_string(), Value::from(2.0)),
            ("health".to_string(), Value::Null),
        ]
    );
    let run = rows(
        &store,
        "SELECT rpo_target_s, rpo_measured_s FROM experiments WHERE experiment_id = 'rpo-1'",
    );
    assert_eq!(run[0]["rpo_target_s"], 1.0);
    assert_eq!(run[0]["rpo_measured_s"], 2.0);
    let verdict = rows(
        &store,
        "SELECT kind, metric, observed, state FROM nfr_verdicts WHERE experiment_id = 'rpo-1'",
    );
    assert_eq!(verdict[0]["kind"], "rpo");
    assert_eq!(verdict[0]["metric"], "probe.data-lost.lost_s");
    assert_eq!(verdict[0]["observed"], 2.0);
    assert_eq!(verdict[0]["state"], "breached");
}

/// `probe_samples` as schema v16 to v22 created it, with one sample, and
/// the version set back to `version`.
fn before_the_reading(version: i64) -> Vec<String> {
    vec![
        "DROP TABLE probe_samples".into(),
        "CREATE TABLE probe_samples (
            experiment_id VARCHAR NOT NULL, probe_name VARCHAR NOT NULL,
            phase VARCHAR NOT NULL, ts_ns BIGINT NOT NULL,
            value DOUBLE NOT NULL, ok BOOLEAN NOT NULL
        )"
        .into(),
        "CREATE INDEX idx_probe_samples_experiment_id ON probe_samples (experiment_id)".into(),
        "INSERT INTO probe_samples VALUES ('old-1', 'health', 'post', 500, 120.0, true)".into(),
        format!("UPDATE schema_meta SET value = {version} WHERE key = 'version'"),
    ]
}

fn assert_an_upgrade_keeps_the_samples(version: i64) {
    let d = tempfile::TempDir::new().unwrap();
    let path = d.path().join("kronika.duckdb");
    {
        let store = Store::open(&path).unwrap();
        let writer = store.writer().unwrap();
        for sql in before_the_reading(version) {
            writer.execute(&sql, []).unwrap();
        }
        assert_eq!(writer.schema_version().unwrap(), version);
    }
    // Opening twice proves the migration is idempotent.
    for _ in 0..2 {
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.writer().unwrap().schema_version().unwrap(),
            CURRENT_SCHEMA_VERSION
        );
    }
    let store = Store::open(&path).unwrap();

    assert_eq!(
        columns_of_probe_samples(&store),
        [
            "experiment_id",
            "probe_name",
            "phase",
            "ts_ns",
            "value",
            "ok",
            "reading"
        ]
    );
    let old = rows(
        &store,
        "SELECT value, ok, reading FROM probe_samples WHERE experiment_id = 'old-1'",
    );
    assert_eq!(old.len(), 1);
    assert_eq!(old[0]["value"], 120.0);
    assert_eq!(old[0]["ok"], true);
    assert_eq!(old[0]["reading"], Value::Null);

    store
        .writer()
        .unwrap()
        .ingest_journal(&read_back("rpo-1"), None)
        .unwrap();
    assert_eq!(readings(&store, "rpo-1")[2].1, Value::from(2.0));
}

#[test]
fn a_v21_lake_gains_the_reading_column_and_keeps_its_samples() {
    assert_an_upgrade_keeps_the_samples(21);
}

#[test]
fn a_v22_lake_gains_the_reading_column_and_keeps_its_samples() {
    assert_an_upgrade_keeps_the_samples(22);
}
