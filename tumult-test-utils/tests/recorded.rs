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

#[test]
fn the_line_that_a_run_started_carries_the_trace_of_that_run() {
    let (_dir, reader, journals) = loaded();

    for journal in &journals[..4] {
        let id = &journal.experiment_id;
        let linked = count(
            &reader,
            &format!(
                "SELECT count(*) AS n FROM logs \
                 WHERE body = 'experiment.started' AND log_attrs['experiment_id'] = '{id}' \
                 AND trace_id IN (SELECT trace_id FROM spans \
                 WHERE span_name = 'resilience.experiment' AND experiment_id = '{id}')"
            ),
        );
        assert_eq!(linked, 1, "{}", journal.experiment_title);
    }
}

/// How many probes of a run its journal has as failed: the checks of the
/// steady state, the probes of the method, and the samples.
fn failed_probes(journal: &tumult_core::types::Journal) -> usize {
    use tumult_core::types::{ActivityStatus, ActivityType};
    let checks = [&journal.steady_state_before, &journal.steady_state_after];
    let checked = checks
        .into_iter()
        .flatten()
        .flat_map(|check| &check.probe_results);
    let probed = journal
        .method_results
        .iter()
        .filter(|step| step.activity_type == ActivityType::Probe);
    let failed = checked
        .chain(probed)
        .filter(|probe| probe.status != ActivityStatus::Succeeded)
        .count();
    failed + journal.probe_samples.iter().filter(|s| !s.ok).count()
}

#[test]
fn a_probe_that_read_a_target_that_is_down_is_no_error_of_its_plugin() {
    let (_dir, reader, journals) = loaded();

    // Two of the runs found their target down: the check after the fault
    // failed, and so did each sample taken while they waited for it to
    // come back. The last run sent no telemetry.
    let sent_telemetry = &journals[..journals.len() - 1];
    let failed_in_the_journals: usize = sent_telemetry.iter().map(failed_probes).sum();
    assert!(failed_in_the_journals > 2, "{failed_in_the_journals}");
    let failed_probes = count(
        &reader,
        "SELECT CAST(coalesce(sum(value), 0) AS BIGINT) AS n FROM metric_sums \
         WHERE metric_name = 'tumult.probes.total' \
         AND outcome_status = 'failure'",
    );
    assert_eq!(usize::try_from(failed_probes), Ok(failed_in_the_journals));
    let plugin_errors = count(
        &reader,
        "SELECT CAST(coalesce(sum(value), 0) AS BIGINT) AS n FROM metric_sums \
         WHERE metric_name = 'tumult.plugin.errors.total'",
    );
    assert_eq!(plugin_errors, 0);
}

#[test]
fn the_span_of_every_step_names_the_plugin_that_served_it() {
    let (_dir, reader, _journals) = loaded();
    let steps = "spans WHERE span_name IN ('resilience.action', 'resilience.probe')";

    let all = count(&reader, &format!("SELECT count(*) AS n FROM {steps}"));
    let named = count(
        &reader,
        &format!("SELECT count(*) AS n FROM {steps} AND plugin_name IS NOT NULL"),
    );

    assert!(all > 0);
    assert_eq!(named, all);
}
