//! A probe with a tolerance is judged wherever it runs.
//!
//! These tests run the `tumult` binary against a real target: a state file
//! that says `ok` or `down`. The fault writes `down`, the probes read the
//! file with `cat`, and the rollback leaves a marker file. Nothing is
//! scripted: the journal and the `JUnit` report are the ones a user gets.
#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Output};

use tumult_core::journal::read_journal;
use tumult_core::types::{ActivityResult, ActivityStatus, ExperimentStatus, Journal, NfrState};

const OUTSIDE: &str = "the output is outside the tolerance";

/// A directory with the state file of the target.
struct Lab {
    dir: tempfile::TempDir,
}

impl Lab {
    /// A target whose state file says `state`.
    fn with_state(state: &str) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("state"), format!("{state}\n")).expect("state file");
        Self { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// `tumult` with `args`, a clean environment and a private store.
    fn tumult(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_tumult"))
            .args(args)
            .current_dir(self.dir.path())
            .env("HOME", self.dir.path())
            .env("TUMULT_LAKE_PATH", self.path("lake.duckdb"))
            .env_remove("RUST_LOG")
            .env_remove("TUMULT_ENVIRONMENT")
            .env_remove("TUMULT_DAEMON_URL")
            .env_remove("OTEL_EXPORTER_OTLP_ENDPOINT")
            .output()
            .expect("run tumult")
    }

    /// Run an experiment whose method is `method` and whose rollback leaves
    /// a marker; the exit code and the journal.
    fn run(&self, method: &str) -> (Option<i32>, Journal) {
        self.run_with(method, &leave_marker())
    }

    /// Run an experiment with this method and these rollbacks.
    fn run_with(&self, method: &str, rollbacks: &str) -> (Option<i32>, Journal) {
        let definition = experiment(method, rollbacks);
        std::fs::write(self.path("experiment.toon"), definition).expect("experiment");
        let output = self.tumult(&[
            "run",
            "experiment.toon",
            "--journal-path",
            "journal.toon",
            "--no-ingest",
        ]);
        let journal = read_journal(&self.path("journal.toon")).unwrap_or_else(|e| {
            panic!(
                "no journal ({e}): {}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code(), journal)
    }

    fn rolled_back(&self) -> bool {
        self.path("rolled-back").exists()
    }
}

/// A step that runs `command` in a shell.
fn step(name: &str, activity_type: &str, command: &str) -> String {
    format!(
        r#"  - name: {name}
    activity_type: {activity_type}
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "{command}"
      timeout_s: 5.0
"#
    )
}

/// A probe that reads the state file and must read `expected`.
fn reads(name: &str, expected: &str) -> String {
    format!(
        r#"{}    tolerance:
      type: regex
      pattern: "^{expected}$"
"#,
        step(name, "probe", "cat state")
    )
}

/// `block`, two spaces deeper.
fn indented(block: &str) -> String {
    let lines: Vec<String> = block.lines().map(|line| format!("  {line}")).collect();
    lines.join("\n") + "\n"
}

/// How many steps a block of steps holds.
fn count(steps: &str) -> usize {
    steps.matches("  - name:").count()
}

/// The steady state is `ok` in the state file.
fn experiment(method: &str, rollbacks: &str) -> String {
    format!(
        r"title: Probe tolerance
description: The fault writes down to a state file and the probes read it

rto_target_s: 30.0

steady_state_hypothesis:
  title: The target says ok
  probes[1]:
{steady}
method[{steps}]:
{method}
rollbacks[{undo}]:
{rollbacks}",
        steady = indented(&reads("steady", "ok")),
        steps = count(method),
        undo = count(rollbacks),
    )
}

fn leave_marker() -> String {
    step("leave-marker", "action", "touch rolled-back")
}

fn inject() -> String {
    step("inject", "action", "echo down > state")
}

fn heal() -> String {
    step("heal", "action", "echo ok > state")
}

fn result(results: &[ActivityResult], name: &str) -> ActivityResult {
    results
        .iter()
        .find(|result| result.name == name)
        .cloned()
        .unwrap_or_else(|| panic!("no step {name}"))
}

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

#[test]
fn the_fault_was_applied_so_recovery_is_still_measured() {
    let lab = Lab::with_state("ok");
    let method = [inject(), reads("during", "ok"), heal()].concat();

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
    let method = [inject(), broken, heal()].concat();

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
