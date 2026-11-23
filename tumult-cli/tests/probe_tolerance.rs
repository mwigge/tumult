//! A probe with a tolerance is judged wherever it runs.
//!
//! These tests run the `tumult` binary against a real target: a state file
//! that says `ok` or `down`. The fault writes `down`, the probes read the
//! file with `cat`, and the rollback leaves a marker file. Nothing is
//! scripted: the journal and the `JUnit` report are the ones a user gets.
#![cfg(unix)]

mod lab;

use lab::{heal, inject, leave_marker, reads, result, step, Lab};
use tumult_core::types::{ActivityStatus, ExperimentStatus, NfrState};

const OUTSIDE: &str = "the output is outside the tolerance";

#[test]
fn a_probe_of_the_method_that_reads_the_wrong_value_deviates_the_run() {
    let lab = Lab::with_state("ok");
    // The probe expects the target to stay ok while the fault is in.
    let method = [inject(), reads("during", "ok"), heal()].concat();

    let (code, journal) = lab.run(&method);

    assert_eq!(code, Some(1));
    assert_eq!(journal.status, ExperimentStatus::Deviated);
    let during = result(&journal.method_results, "during");
    assert_eq!(during.status, ActivityStatus::Failed);
    assert_eq!(during.output.as_deref(), Some("down"));
    assert_eq!(during.error.as_deref(), Some(OUTSIDE));
    assert!(lab.rolled_back(), "a deviation gets its rollback");
}

/// A step that keeps the fault in for a second and a half: the steady
/// state, read once a second, sees the outage.
fn hold() -> String {
    step("hold", "action", "sleep 1.5")
}

#[test]
fn the_fault_was_applied_so_recovery_is_still_measured() {
    let lab = Lab::with_state("ok");
    let method = [inject(), reads("during", "ok"), hold(), heal()].concat();

    let (_, journal) = lab.run(&method);

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    assert_eq!(journal.hypothesis_met(), Some(true));
    assert!(journal.recovery_time_s().is_some());
    let objectives = journal.objectives.expect("the run has a recovery target");
    assert_eq!(objectives.verdicts[0].nfr_id, "rto");
    assert_eq!(objectives.verdicts[0].state, NfrState::Met);
}

#[test]
fn a_probe_of_the_method_that_reads_what_it_expects_completes_the_run() {
    let lab = Lab::with_state("ok");
    // The probe checks that the fault is visible.
    let method = [inject(), reads("during", "down"), heal()].concat();

    let (code, journal) = lab.run(&method);

    assert_eq!(code, Some(0));
    assert_eq!(journal.status, ExperimentStatus::Completed);
    let during = result(&journal.method_results, "during");
    assert_eq!(during.status, ActivityStatus::Succeeded);
    assert_eq!(during.error, None);
    assert!(!lab.rolled_back(), "nothing deviated");
}

#[test]
fn a_probe_of_the_method_that_cannot_run_deviates_the_run() {
    let lab = Lab::with_state("ok");
    let broken = step("during", "probe", "exit 3");
    let method = [inject(), broken, hold(), heal()].concat();

    let (code, journal) = lab.run(&method);

    assert_eq!(code, Some(1));
    assert_eq!(journal.status, ExperimentStatus::Deviated);
    let during = result(&journal.method_results, "during");
    assert_eq!(during.status, ActivityStatus::Failed);
    assert_ne!(during.error.as_deref(), Some(OUTSIDE));
    assert!(journal.recovery_time_s().is_some());
}

#[test]
fn an_action_that_fails_makes_the_run_failed_and_measures_no_recovery() {
    let lab = Lab::with_state("ok");
    let broken = step("inject", "action", "exit 3");
    let method = [broken, reads("during", "ok")].concat();

    let (code, journal) = lab.run(&method);

    assert_eq!(code, Some(1));
    assert_eq!(journal.status, ExperimentStatus::Failed);
    assert_eq!(journal.recovery_time_s(), None);
}

#[test]
fn a_probe_of_the_steady_state_that_reads_the_wrong_value_is_a_failed_step() {
    let lab = Lab::with_state("down");

    let (code, journal) = lab.run(&inject());

    assert_eq!(code, Some(1));
    assert_eq!(journal.status, ExperimentStatus::Aborted);
    assert!(journal.method_results.is_empty(), "the method did not run");
    let before = journal.steady_state_before.expect("the check ran");
    assert!(!before.met);
    let steady = result(&before.probe_results, "steady");
    assert_eq!(steady.status, ActivityStatus::Failed);
    assert_eq!(steady.output.as_deref(), Some("down"));
    assert_eq!(steady.error.as_deref(), Some(OUTSIDE));
}

#[test]
fn a_rollback_probe_that_reads_the_wrong_value_is_a_failed_rollback() {
    let lab = Lab::with_state("ok");
    let method = [inject(), reads("during", "ok"), heal()].concat();
    // The check of the rollback expects a value the target does not have.
    let rollbacks = [leave_marker(), reads("restored", "restored")].concat();

    let (_, journal) = lab.run_with(&method, &rollbacks);

    let restored = result(&journal.rollback_results, "restored");
    assert_eq!(restored.status, ActivityStatus::Failed);
    assert_eq!(restored.output.as_deref(), Some("ok"));
    assert_eq!(restored.error.as_deref(), Some(OUTSIDE));
    assert_eq!(journal.rollback_failures, 1);
}

#[test]
fn a_rollback_probe_that_reads_what_it_expects_is_not_a_failure() {
    let lab = Lab::with_state("ok");
    let method = [inject(), reads("during", "ok"), heal()].concat();
    let rollbacks = [leave_marker(), reads("restored", "ok")].concat();

    let (_, journal) = lab.run_with(&method, &rollbacks);

    let restored = result(&journal.rollback_results, "restored");
    assert_eq!(restored.status, ActivityStatus::Succeeded);
    assert_eq!(journal.rollback_failures, 0);
}

#[test]
fn the_junit_report_of_the_run_fails_the_probe_that_did_not_hold() {
    let lab = Lab::with_state("ok");
    let method = [inject(), reads("during", "ok"), heal()].concat();
    lab.run(&method);

    let output = lab.tumult(&[
        "report",
        "journal.toon",
        "--format",
        "junit",
        "--output",
        "report.xml",
    ]);

    assert_eq!(output.status.code(), Some(0));
    let xml = std::fs::read_to_string(lab.path("report.xml")).expect("report");
    assert!(xml.contains(r#"failures="1""#), "{xml}");
    let case = xml
        .split("<testcase ")
        .find(|case| case.starts_with(r#"name="during""#))
        .unwrap_or_else(|| panic!("no case for the probe: {xml}"));
    assert!(
        case.contains(r#"<failure message="Failed">the output is outside the tolerance</failure>"#),
        "{case}"
    );
}
