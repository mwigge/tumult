//! How a run ends, as the `tumult` binary reports it.
//!
//! Each test runs the binary against the lab: a state file that real `sh`
//! steps read and write. A run says how it ended in four places, and the
//! tests read all four: the exit code, the `Status:` line on stdout, the
//! journal, and the `experiment.completed` event on stderr.
#![cfg(unix)]

mod lab;

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use lab::{
    experiment, heal, inject, leave_marker, reads, result, step, step_within, wait_for, Lab,
};
use serde_json::Value;
use tumult_core::types::{ActivityStatus, ExperimentStatus, Journal};

/// A guard that holds while the state file says `ok`.
const GUARD: &str = r#"
guards[1]:
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
"#;

/// A step that would run for a minute. It leaves the process id of its
/// `sleep` in `sleep.pid`, then runs `then`, then leaves `started`.
fn long_step(name: &str, then: &str) -> String {
    let command = format!("sleep 60 & echo $! > sleep.pid; {then} touch started; wait");
    step_within(name, "action", &command, 90.0)
}

/// What the binary said about one run.
struct Ending {
    code: Option<i32>,
    stdout: String,
    /// The `experiment.completed` events on stderr.
    completed: Vec<Value>,
    journal: Journal,
    took: Duration,
}

/// The events on stderr whose message is `message`.
fn events(stderr: &[u8], message: &str) -> Vec<Value> {
    String::from_utf8_lossy(stderr)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|event| event["message"] == message)
        .collect()
}

/// `command`, with the events of the runner on stderr.
fn logged(mut command: Command) -> Command {
    command
        .env("RUST_LOG", "tumult_core=info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn ending(lab: &Lab, output: &Output, started: Instant) -> Ending {
    Ending {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        completed: events(&output.stderr, "experiment.completed"),
        journal: lab.journal(output),
        took: started.elapsed(),
    }
}

/// Run `definition` to its end.
fn run(lab: &Lab, definition: &str) -> Ending {
    let started = Instant::now();
    let output = logged(lab.run_command(definition))
        .output()
        .expect("run tumult");
    ending(lab, &output, started)
}

/// Start `definition`, and send the binary SIGINT once its long step runs.
fn run_and_interrupt(lab: &Lab, definition: &str) -> Ending {
    let started = Instant::now();
    let child = logged(lab.run_command(definition))
        .spawn()
        .expect("start tumult");
    wait_for(&lab.path("started"));
    let killed = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("send SIGINT");
    assert!(killed.success(), "SIGINT was not sent");
    let output = child.wait_with_output().expect("wait for tumult");
    ending(lab, &output, started)
}

/// Whether the `sleep` of the long step is still running.
fn sleep_is_running(lab: &Lab) -> bool {
    let pid = std::fs::read_to_string(lab.path("sleep.pid")).expect("sleep.pid");
    Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(Stdio::null())
        .status()
        .expect("probe the process")
        .success()
}

/// All four places say the run ended as `status`, spelled `word` in the log.
fn assert_ended(ending: &Ending, status: &ExperimentStatus, word: &str) {
    assert_eq!(&ending.journal.status, status);
    let code = i32::from(*status != ExperimentStatus::Completed);
    assert_eq!(ending.code, Some(code), "{}", ending.stdout);
    let line = format!("Status: {status:?}\n");
    assert!(ending.stdout.contains(&line), "{}", ending.stdout);
    assert_eq!(ending.completed.len(), 1, "{:?}", ending.completed);
    let event = &ending.completed[0];
    assert_eq!(event["status"], word);
    assert_eq!(
        event["experiment_id"],
        ending.journal.experiment_id.as_str()
    );
}

#[test]
fn a_run_whose_target_comes_back_ends_completed() {
    let lab = Lab::with_state("ok");
    let method = [inject(), heal()].concat();

    let ending = run(&lab, &experiment(&method, &leave_marker()));

    assert_ended(&ending, &ExperimentStatus::Completed, "completed");
    assert_eq!(ending.completed[0]["deviations"], 0);
    assert!(!lab.rolled_back(), "a completed run needs no rollback");
}

#[test]
fn a_run_whose_probe_does_not_hold_ends_deviated() {
    let lab = Lab::with_state("ok");
    let method = [inject(), reads("during", "ok"), heal()].concat();

    let ending = run(&lab, &experiment(&method, &leave_marker()));

    assert_ended(&ending, &ExperimentStatus::Deviated, "deviated");
    assert_eq!(ending.completed[0]["deviations"], 1);
    assert!(lab.rolled_back());
}

#[test]
fn a_run_whose_action_fails_ends_failed() {
    let lab = Lab::with_state("ok");
    let method = step("inject", "action", "exit 3");

    let ending = run(&lab, &experiment(&method, &leave_marker()));

    assert_ended(&ending, &ExperimentStatus::Failed, "failed");
    assert_eq!(ending.completed[0]["deviations"], 0);
    assert!(lab.rolled_back());
}

#[test]
fn a_run_whose_target_is_down_before_the_fault_ends_aborted() {
    let lab = Lab::with_state("down");
    let method = step("inject", "action", "touch fault-applied");

    let ending = run(&lab, &experiment(&method, &leave_marker()));

    assert_ended(&ending, &ExperimentStatus::Aborted, "aborted");
    assert!(!lab.path("fault-applied").exists(), "the fault was applied");
}

#[test]
fn a_guard_that_breaks_ends_the_run_halted_and_stops_the_step_that_runs() {
    let lab = Lab::with_state("ok");
    // The guard can stop the step as soon as the state is down, so the step
    // takes the target down once `sleep.pid` is written.
    let method = [
        long_step("inject", "echo down > state;"),
        step("next", "action", "touch next-step-ran"),
    ]
    .concat();
    let definition = [experiment(&method, &leave_marker()).as_str(), GUARD].concat();

    let ending = run(&lab, &definition);

    assert_ended(&ending, &ExperimentStatus::Halted, "halted");
    let halt = ending
        .journal
        .halt
        .as_ref()
        .expect("the journal has the halt");
    assert_eq!(halt.guard_name, "state-guard");
    assert!(lab.rolled_back(), "a halt gets its rollback");
    assert!(!lab.path("next-step-ran").exists(), "the method went on");
    assert!(
        ending.took < Duration::from_secs(30),
        "the halt waited for the step: {:?}",
        ending.took
    );
    let inject = result(&ending.journal.method_results, "inject");
    assert_eq!(inject.status, ActivityStatus::Failed);
    let error = inject.error.unwrap_or_default();
    assert!(error.contains("stopped before it finished"), "{error}");
    assert!(!sleep_is_running(&lab), "the step left a process behind");
}

#[test]
fn a_run_that_gets_sigint_ends_interrupted_and_stops_the_step_that_runs() {
    let lab = Lab::with_state("ok");
    let method = [
        long_step("inject", ""),
        step("next", "action", "touch next-step-ran"),
    ]
    .concat();

    let ending = run_and_interrupt(&lab, &experiment(&method, &leave_marker()));

    assert_ended(&ending, &ExperimentStatus::Interrupted, "interrupted");
    assert!(lab.rolled_back(), "an interrupted run gets its rollback");
    assert!(!lab.path("next-step-ran").exists(), "the method went on");
    assert!(
        ending.took < Duration::from_secs(30),
        "the interrupt waited for the step: {:?}",
        ending.took
    );
    let inject = result(&ending.journal.method_results, "inject");
    assert_eq!(inject.status, ActivityStatus::Failed);
    let error = inject.error.unwrap_or_default();
    assert!(error.contains("stopped before it finished"), "{error}");
    assert!(!sleep_is_running(&lab), "the step left a process behind");
}
