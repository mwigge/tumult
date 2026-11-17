//! The recording, loaded into a store.
//!
//! The runs are the ones the example `record_telemetry` plans: a run that
//! holds, two runs that find a weakness, the second of them run again
//! after its fix, and a run that sent no telemetry.
#![cfg(feature = "recorded")]

use tumult_core::types::ExperimentStatus;
use tumult_lake::{Reader, Store};
use tumult_test_utils::recorded;

/// A store with the recording, and the journals of its runs.
fn loaded() -> (tempfile::TempDir, Reader, Vec<tumult_core::types::Journal>) {
    let dir = tempfile::tempdir().expect("a directory");
    let store = Store::open(&dir.path().join("lake.duckdb")).expect("a store");
    let journals = recorded::load(&store.writer().expect("a writer"));
    let reader = store.read_only().expect("a reader");
    (dir, reader, journals)
}

/// The one number the query `sql` gives.
fn count(reader: &Reader, sql: &str) -> i64 {
    let rows = reader.query_json_rows(sql).expect("the query");
    rows[0]["n"].as_i64().expect("a number")
}

#[test]
fn the_journals_are_those_of_the_planned_runs_in_their_order() {
    let (_dir, _reader, journals) = loaded();

    let runs: Vec<(&str, ExperimentStatus)> = journals
        .iter()
        .map(|journal| (journal.experiment_title.as_str(), journal.status.clone()))
        .collect();

    assert_eq!(
        runs,
        [
            (
                "Checkout survives a cache restart",
                ExperimentStatus::Completed
            ),
            (
                "Payments tolerate a slow ledger",
                ExperimentStatus::Deviated
            ),
            (
                "Search answers after the loss of a node",
                ExperimentStatus::Deviated
            ),
            (
                "Search answers after the loss of a node",
                ExperimentStatus::Completed
            ),
            (
                "Billing export resumes after a restart",
                ExperimentStatus::Completed
            ),
        ]
    );
    let mut starts: Vec<i64> = journals.iter().map(|j| j.started_at_ns).collect();
    starts.sort_unstable();
    assert_eq!(
        starts,
        journals.iter().map(|j| j.started_at_ns).collect::<Vec<_>>()
    );
}

#[test]
fn every_run_is_in_the_store_with_its_steps() {
    let (_dir, reader, journals) = loaded();

    assert_eq!(count(&reader, "SELECT count(*) AS n FROM experiments"), 5);
    for journal in &journals {
        let steps = journal.method_results.len() + journal.rollback_results.len();
        let stored = count(
            &reader,
            &format!(
                "SELECT count(*) AS n FROM activity_results \
                 WHERE experiment_id = '{}' AND phase IN ('method', 'rollback')",
                journal.experiment_id
            ),
        );
        assert_eq!(
            stored,
            i64::try_from(steps).expect("a count"),
            "{}",
            journal.experiment_title
        );
    }
}

#[test]
fn the_runs_that_sent_telemetry_have_their_trace_their_logs_and_their_metrics() {
    let (_dir, reader, journals) = loaded();

    for journal in &journals[..4] {
        let id = &journal.experiment_id;
        let roots = count(
            &reader,
            &format!(
                "SELECT count(*) AS n FROM spans \
                 WHERE span_name = 'resilience.experiment' AND experiment_id = '{id}'"
            ),
        );
        assert_eq!(roots, 1, "{}", journal.experiment_title);
        let ended = count(
            &reader,
            &format!(
                "SELECT count(*) AS n FROM logs \
                 WHERE body = 'experiment.completed' AND log_attrs['experiment_id'] = '{id}'"
            ),
        );
        assert_eq!(ended, 1, "{}", journal.experiment_title);
    }
    // Each run counts itself once.
    let counted = count(
        &reader,
        "SELECT CAST(sum(value) AS BIGINT) AS n FROM metric_sums \
         WHERE metric_name = 'tumult.experiments.total'",
    );
    assert_eq!(counted, 4);
}

#[test]
fn the_run_that_sent_no_telemetry_has_only_its_journal() {
    let (_dir, reader, journals) = loaded();
    let id = &journals[4].experiment_id;

    for rows_of_the_run in [
        format!("spans WHERE experiment_id = '{id}'"),
        format!("logs WHERE log_attrs['experiment_id'] = '{id}'"),
    ] {
        let rows = count(
            &reader,
            &format!("SELECT count(*) AS n FROM {rows_of_the_run}"),
        );
        assert_eq!(rows, 0, "{rows_of_the_run}");
    }
}
