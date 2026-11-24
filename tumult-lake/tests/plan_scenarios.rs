//! Schema v26: the plan scenarios of RTS 2024/1774 a test names. A run
//! keeps the scenarios of its regulatory block, a manual record keeps the
//! scenarios it was written with, each checked against the closed set on
//! write and sealed. A lake that v25 left opens with the columns added and
//! its rows kept.

#![cfg(feature = "duckdb")]

use serde_json::{json, Value};
use tumult_core::types::{ExperimentStatus, Journal, RegulatoryMapping};
use tumult_lake::duckdb_store::sample_journal;
use tumult_lake::{
    ExerciseDetail, ExerciseType, ManualError, ManualOutcome, NewManualExperiment, Store,
    CURRENT_SCHEMA_VERSION,
};

const DAY: i64 = 86_400 * 1_000_000_000;

fn temp_store() -> (tempfile::TempDir, Store) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    (d, store)
}

/// A run whose regulatory block names `scenarios`.
fn tagged_run(id: &str, scenarios: &[&str]) -> Journal {
    let mut journal = sample_journal(id, ExperimentStatus::Completed);
    journal.regulatory = Some(RegulatoryMapping {
        frameworks: vec!["DORA".into()],
        requirements: vec![],
        tester: None,
        scenarios: scenarios.iter().map(|s| (*s).to_owned()).collect(),
    });
    journal
}

fn record(scenarios: &[&str]) -> NewManualExperiment {
    NewManualExperiment {
        experiment_name: "payments site loss".into(),
        exercise_type: ExerciseType::Failover,
        executed_at_ns: 100 * DAY,
        hypothesis: "Payments run from the second site within the RTO".into(),
        method: "Cut the primary data centre off and fail over".into(),
        outcome: ManualOutcome::Passed,
        hypothesis_met: Some(true),
        findings: None,
        action_items: vec![],
        target_system: Some("payments".into()),
        target_environment: Some("production".into()),
        blast_radius: None,
        recovery_time_s: Some(900.0),
        duration_s: None,
        entered_by: "alice".into(),
        attestation: "I attest this record reflects the test as executed.".into(),
        renewal_due_ns: None,
        framework_refs: vec!["rts1774.art26".into()],
        exercise: ExerciseDetail {
            scenarios: scenarios.iter().map(|s| (*s).to_owned()).collect(),
            ..ExerciseDetail::default()
        },
    }
}

fn scenarios_of_run(store: &Store, id: &str) -> Value {
    let rows = store
        .read_only()
        .unwrap()
        .query_json_rows_bound(
            "SELECT dora_scenarios FROM experiments WHERE experiment_id = $1",
            &[id],
        )
        .unwrap();
    assert_eq!(rows.len(), 1, "{id}: {rows:?}");
    rows[0]["dora_scenarios"].clone()
}

fn detail(store: &Store, id: &str) -> tumult_lake::ManualDetail {
    store
        .read_only()
        .unwrap()
        .manual_experiment_detail(id)
        .unwrap()
        .unwrap()
}

fn count(store: &Store, table: &str) -> i64 {
    store
        .read_only()
        .unwrap()
        .query_json_rows(&format!("SELECT count(*) AS n FROM {table}"))
        .unwrap()[0]["n"]
        .as_i64()
        .unwrap()
}

#[test]
fn a_run_keeps_the_scenarios_of_its_regulatory_block() {
    let (_d, store) = temp_store();
    let writer = store.writer().unwrap();
    for journal in [
        tagged_run("site-loss", &["premises_loss", "power_outage"]),
        tagged_run("mapped-only", &[]),
        sample_journal("plain", ExperimentStatus::Completed),
    ] {
        assert!(writer.ingest_journal(&journal, None).unwrap());
    }

    assert_eq!(
        scenarios_of_run(&store, "site-loss"),
        json!(["premises_loss", "power_outage"])
    );
    // A block that names none, and a run with no block, keep nothing.
    assert_eq!(scenarios_of_run(&store, "mapped-only"), Value::Null);
    assert_eq!(scenarios_of_run(&store, "plain"), Value::Null);
}

#[test]
fn a_record_keeps_the_scenarios_it_was_written_with_and_an_edit_replaces_them() {
    let (_d, store) = temp_store();
    let writer = store.writer().unwrap();
    let id = writer
        .create_manual_draft(&record(&["premises_loss", "redundant_facility"]))
        .unwrap();
    assert_eq!(
        detail(&store, &id).experiment["scenarios"],
        json!(["premises_loss", "redundant_facility"])
    );

    writer
        .update_manual_draft(&id, &record(&["power_outage"]), "alice")
        .unwrap();
    assert_eq!(
        detail(&store, &id).experiment["scenarios"],
        json!(["power_outage"])
    );
    writer
        .update_manual_draft(&id, &record(&[]), "alice")
        .unwrap();
    assert_eq!(detail(&store, &id).experiment["scenarios"], json!([]));
}

#[test]
fn a_scenario_the_rts_does_not_name_is_refused_with_the_nearest_id() {
    let (_d, store) = temp_store();
    let writer = store.writer().unwrap();
    let err = writer
        .create_manual_draft(&record(&["premises_loss", "power_outages"]))
        .unwrap_err();
    let ManualError::UnknownScenario(unknown) = &err else {
        panic!("{err:?}");
    };
    assert_eq!(unknown.id, "power_outages");
    assert_eq!(unknown.suggestion, Some("power_outage"));
    assert_eq!(
        err.to_string(),
        "scenarios: 'power_outages' is not a scenario id of RTS 2024/1774 Arts. 25, 26 and 39; \
         did you mean 'power_outage'?"
    );
    assert_eq!(count(&store, "manual_experiments"), 0);

    let id = writer.create_manual_draft(&record(&[])).unwrap();
    let edit = writer.update_manual_draft(&id, &record(&["fire"]), "alice");
    assert!(
        matches!(edit, Err(ManualError::UnknownScenario(_))),
        "{edit:?}"
    );
}

#[test]
fn the_seal_covers_the_scenarios() {
    let (_d, store) = temp_store();
    let writer = store.writer().unwrap();
    let id = writer
        .create_manual_draft(&record(&["premises_loss"]))
        .unwrap();
    writer.submit_manual(&id, None, "alice").unwrap();
    writer.verify_manual(&id, "bob", None).unwrap();
    let ok = || {
        store
            .read_only()
            .unwrap()
            .verify_manual_record(&id)
            .unwrap()
            .unwrap()
            .ok
    };
    assert!(ok());
    writer
        .execute(
            "UPDATE manual_experiments SET scenarios = ['power_outage'] WHERE id = ?",
            [&id],
        )
        .unwrap();
    assert!(!ok(), "a changed scenario must break the seal");
}

/// The columns `dora_scenarios` and `scenarios`, as a v25 lake lacks them,
/// and the version set back to 25.
fn a_v25_lake(path: &std::path::Path) -> String {
    let store = Store::open(path).unwrap();
    let writer = store.writer().unwrap();
    assert!(writer
        .ingest_journal(
            &sample_journal("old-run", ExperimentStatus::Completed),
            None
        )
        .unwrap());
    let id = writer.create_manual_draft(&record(&[])).unwrap();
    // DuckDB drops no column of a table with an index: the unique index of
    // `experiments` goes and comes back around the drop.
    for sql in [
        "DROP INDEX idx_experiments_id",
        "ALTER TABLE experiments DROP COLUMN dora_scenarios",
        "CREATE UNIQUE INDEX idx_experiments_id ON experiments (experiment_id)",
        "ALTER TABLE manual_experiments DROP COLUMN scenarios",
        "UPDATE schema_meta SET value = 25 WHERE key = 'version'",
    ] {
        writer.execute(sql, []).unwrap();
    }
    assert_eq!(writer.schema_version().unwrap(), 25);
    id
}

#[test]
fn a_v25_lake_gains_the_scenario_columns_and_keeps_its_runs_and_records() {
    let d = tempfile::TempDir::new().unwrap();
    let path = d.path().join("kronika.duckdb");
    let old = a_v25_lake(&path);

    // Opening twice proves the migration is idempotent.
    for _ in 0..2 {
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.writer().unwrap().schema_version().unwrap(),
            CURRENT_SCHEMA_VERSION
        );
    }
    assert_eq!(CURRENT_SCHEMA_VERSION, 26);
    let store = Store::open(&path).unwrap();

    // The rows stored before know of no scenario.
    assert_eq!(scenarios_of_run(&store, "old-run"), Value::Null);
    let before = detail(&store, &old);
    assert_eq!(before.experiment["scenarios"], Value::Null);
    assert_eq!(before.experiment["status"], "draft");

    let writer = store.writer().unwrap();
    assert!(writer
        .ingest_journal(&tagged_run("new-run", &["cyber_attack"]), None)
        .unwrap());
    assert_eq!(scenarios_of_run(&store, "new-run"), json!(["cyber_attack"]));
    let id = writer
        .create_manual_draft(&record(&["third_party_failure"]))
        .unwrap();
    assert_eq!(
        detail(&store, &id).experiment["scenarios"],
        json!(["third_party_failure"])
    );
}
