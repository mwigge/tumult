//! What `tumult run` prints when a run has ended.
//!
//! Each test runs the binary against the lab: a state file that real `sh`
//! steps read and write. The whole of stdout is read, so a line that is
//! missing, one too many or out of place fails the test. The two values
//! that differ from run to run come from the journal the run wrote: how
//! long it took, and the hash of the file.
#![cfg(unix)]

mod lab;

use lab::{experiment, heal, inject, leave_marker, step, step_within, with_block, Lab};
use sha2::{Digest, Sha256};
use tumult_core::types::ExperimentStatus;

/// A run of the lab as the binary reported it.
struct Printed {
    code: Option<i32>,
    stdout: String,
    status: ExperimentStatus,
    /// The lines every run prints after its status and before its summary.
    duration: String,
    /// The two lines every run ends with.
    journal: String,
}

/// Run `definition` with the stand-in in the place of `oha`.
fn run(lab: &Lab, definition: &str) -> Printed {
    let output = lab
        .run_command(definition)
        .env("TUMULT_OHA_BINARY", lab.oha_stand_in())
        .output()
        .expect("run tumult");
    let journal = lab.journal(&output);
    let file = std::fs::read(lab.path("journal.toon")).expect("journal file");
    let hash: String = Sha256::digest(&file)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<String>>()
        .concat();
    Printed {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        status: journal.status,
        duration: format!("Duration: {}ms\n", journal.duration_ms),
        journal: format!("Journal written to: journal.toon\nJournal SHA-256: {hash}\n"),
    }
}

/// The experiment of the lab without its recovery target, so that the run
/// has no objective.
fn without_objectives(method: &str) -> String {
    let target = "rto_target_s: 30.0\n";
    let definition = experiment(method, &leave_marker());
    assert!(definition.contains(target), "{definition}");
    definition.replacen(target, "", 1)
}

/// What the binary prints for a run of the lab: `status`, then `steps`
/// lines about the steps, then `summary`.
fn expected(printed: &Printed, status: &str, steps: &str, summary: &str) -> String {
    [
        "Running experiment: Probe tolerance\n",
        &format!("Status: {status}\n"),
        &printed.duration,
        steps,
        summary,
        &printed.journal,
    ]
    .concat()
}

#[test]
fn a_run_that_holds_says_that_its_steady_state_was_met_before_and_after() {
    let lab = Lab::with_state("ok");
    let method = [inject(), heal()].concat();

    let printed = run(&lab, &without_objectives(&method));

    assert_eq!(printed.status, ExperimentStatus::Completed);
    assert_eq!(printed.code, Some(0));
    assert_eq!(
        printed.stdout,
        expected(
            &printed,
            "Completed",
            "Method steps: 2 executed\n",
            "Hypothesis: met before, met after\n",
        )
    );
}

#[test]
fn a_run_whose_target_stays_down_says_that_its_steady_state_was_not_met_after() {
    let lab = Lab::with_state("ok");

    let printed = run(&lab, &without_objectives(&inject()));

    assert_eq!(printed.status, ExperimentStatus::Deviated);
    assert_eq!(printed.code, Some(1));
    assert_eq!(
        printed.stdout,
        expected(
            &printed,
            "Deviated",
            "Method steps: 1 executed\nRollbacks: 1 executed\n",
            "Hypothesis: met before, not met after\n",
        )
    );
}

/// `value` as the summary writes a number: three decimals at most.
fn written(value: f64) -> String {
    let fixed = format!("{value:.3}");
    fixed
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

#[test]
fn a_run_whose_target_never_came_back_says_its_rto_was_breached_and_not_recovered() {
    let lab = Lab::with_state("ok");

    let printed = run(&lab, &experiment(&inject(), &leave_marker()));

    let journal = tumult_core::journal::read_journal(&lab.path("journal.toon")).expect("journal");
    let rto = &journal.objectives.expect("a recovery target").verdicts[0];
    let watched = rto.observed.map_or_else(String::new, written);
    assert_eq!(printed.status, ExperimentStatus::Deviated);
    assert_eq!(printed.code, Some(1));
    assert_eq!(
        printed.stdout,
        expected(
            &printed,
            "Deviated",
            "Method steps: 1 executed\nRollbacks: 1 executed\n",
            &format!(
                "Hypothesis: met before, not met after\n\
                 Objectives: 0 met, 1 breached, 0 inconclusive\n\
                 \x20 breached: rto: recovery.mttr_s must be at most 30, \
                 observed at least {watched}, not recovered\n"
            ),
        )
    );
}

#[test]
fn a_run_that_starts_on_a_target_that_is_down_says_that_nothing_was_checked_after() {
    let lab = Lab::with_state("down");

    let printed = run(&lab, &without_objectives(&inject()));

    assert_eq!(printed.status, ExperimentStatus::Aborted);
    assert_eq!(printed.code, Some(1));
    assert_eq!(
        printed.stdout,
        expected(
            &printed,
            "Aborted",
            "Method steps: 0 executed\nRollbacks: 1 executed\n",
            "Hypothesis: not met before, not checked after\n",
        )
    );
}

/// An experiment that only acts: it names no steady state.
const NO_STEADY_STATE: &str = r#"title: Probe tolerance
description: A run that only acts

method[1]:
  - name: act
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "true"
      timeout_s: 5.0
"#;

#[test]
fn a_run_without_a_steady_state_has_no_line_about_one() {
    let lab = Lab::with_state("ok");

    let printed = run(&lab, NO_STEADY_STATE);

    assert_eq!(printed.status, ExperimentStatus::Completed);
    assert_eq!(
        printed.stdout,
        expected(&printed, "Completed", "Method steps: 1 executed\n", "")
    );
}

/// Six objectives and a load. The stand-in for `oha` answers 40 requests
/// without an error, with a p95 of 80 ms and a p99 of 100 ms: the first
/// and the last objective are met, the two between them are not. The
/// recovery time of the lab is met. Nothing in the lab measures lost data,
/// so the recovery point has no verdict. No two of the three counts are
/// the same.
const OBJECTIVES_AND_LOAD: &str = r#"rpo_target_s: 5.0

nfr[4]{id,kind,metric,operator,threshold}:
  fast-enough,latency,load.latency_p95_ms,lte,200.0
  many-requests,throughput,load.total_requests,gte,100.0
  short-tail,latency,load.latency_p99_ms,lte,50.0
  no-errors,error_rate,load.error_rate,lte,0.0

load:
  tool: oha
  script: "http://127.0.0.1:9/health"
  vus: 2
  duration_s: 2.0
"#;

#[test]
fn a_run_counts_its_objectives_and_names_each_one_that_was_not_met() {
    let lab = Lab::with_state("ok");
    let method = [
        step("inject", "action", "echo down > state; sleep 1"),
        heal(),
    ]
    .concat();
    let definition = with_block(&experiment(&method, &leave_marker()), OBJECTIVES_AND_LOAD);

    let printed = run(&lab, &definition);

    assert_eq!(printed.status, ExperimentStatus::Completed);
    assert_eq!(
        printed.stdout,
        expected(
            &printed,
            "Completed",
            "Method steps: 2 executed\n",
            "Hypothesis: met before, met after\n\
             Objectives: 3 met, 2 breached, 1 inconclusive\n\
             \x20 breached: many-requests: load.total_requests must be at least 100, observed 40\n\
             \x20 breached: short-tail: load.latency_p99_ms must be at most 50, observed 100\n\
             \x20 inconclusive: rpo: rpo must be at most 5, not measured\n\
             Load (oha): 40 requests, 13.3 requests/s, p50 20.0 ms, p95 80.0 ms, \
             p99 100.0 ms, error rate 0.00 %, thresholds met\n",
        )
    );
}

/// A guard that holds while the state file says `ok`, and a load of a
/// minute.
const GUARD_AND_LOAD: &str = r#"guards[1]:
  - name: state-guard
    probe:
      name: state-watch
      activity_type: probe
      provider:
        type: process
        path: sh
        arguments[2]: "-c", "cat state"
        timeout_s: 5.0
      tolerance:
        type: regex
        pattern: "^ok$"
    min_breaches: 1

load:
  tool: oha
  script: "http://127.0.0.1:9/health"
  vus: 2
  duration_s: 60.0
"#;

#[test]
fn a_halted_run_says_that_its_load_was_cut() {
    let lab = Lab::with_state("ok");
    // The step would run for a minute. It breaks the guard at once.
    let method = step_within("inject", "action", "echo down > state; sleep 60", 90.0);
    let definition = with_block(&without_objectives(&method), GUARD_AND_LOAD);

    let printed = run(&lab, &definition);

    assert_eq!(printed.status, ExperimentStatus::Halted);
    assert_eq!(printed.code, Some(1));
    assert_eq!(
        printed.stdout,
        expected(
            &printed,
            "Halted",
            "Method steps: 1 executed\nRollbacks: 1 executed\n",
            "Hypothesis: met before, not checked after\n\
             Load (oha): 40 requests, 13.3 requests/s, p50 20.0 ms, p95 80.0 ms, \
             p99 100.0 ms, error rate 0.00 %, thresholds not met, \
             cut before its planned end\n",
        )
    );
}
