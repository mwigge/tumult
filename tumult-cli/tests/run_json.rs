//! What `tumult run --output-format json` prints.
//!
//! Each test runs the binary against the lab and reads its two streams.
//! Stdout is for the program that reads the journal: it holds the journal
//! and nothing else. What the run says to a person is on stderr.
#![cfg(unix)]

mod lab;

use std::process::Output;

use lab::{experiment, fatal_error, heal, inject, leave_marker, Lab};
use serde_json::Value;

/// Run `definition` and ask for the journal as JSON.
fn run_for_json(lab: &Lab, definition: &str) -> Output {
    lab.run_command(definition)
        .args(["--output-format", "json"])
        .output()
        .expect("run tumult")
}

/// The journal file of the lab as the JSON text the binary prints for it.
fn journal_as_json(lab: &Lab, output: &Output) -> String {
    let json = serde_json::to_string_pretty(&lab.journal(output)).expect("journal as JSON");
    format!("{json}\n")
}

/// The status in the JSON on `stdout`. Stdout that is not one JSON value
/// fails the test.
fn status_on(stdout: &str) -> Value {
    let journal: Value = serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{stdout}"));
    journal["status"].clone()
}

#[test]
fn a_run_asked_for_json_prints_its_journal_and_nothing_else_on_stdout() {
    let lab = Lab::with_state("ok");
    let method = [inject(), heal()].concat();

    let output = run_for_json(&lab, &experiment(&method, &leave_marker()));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert_eq!(stdout, journal_as_json(&lab, &output));
    assert_eq!(status_on(&stdout), "completed");
}

#[test]
fn a_run_asked_for_json_says_the_rest_on_stderr() {
    let lab = Lab::with_state("ok");
    let method = [inject(), heal()].concat();

    let output = run_for_json(&lab, &experiment(&method, &leave_marker()));

    let stderr = String::from_utf8_lossy(&output.stderr);
    for lines in [
        "Running experiment: Probe tolerance\nStatus: Completed\n",
        "Method steps: 2 executed\nHypothesis: met before, met after\n",
        "Journal written to: journal.toon\nJournal SHA-256: ",
    ] {
        assert!(stderr.contains(lines), "no {lines:?} in:\n{stderr}");
    }
}

#[test]
fn a_run_that_did_not_complete_still_prints_its_journal() {
    let lab = Lab::with_state("down");

    let output = run_for_json(&lab, &experiment(&inject(), &leave_marker()));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{stdout}");
    assert_eq!(
        fatal_error(&output.stderr),
        "experiment finished with status: Aborted"
    );
    assert_eq!(stdout, journal_as_json(&lab, &output));
    assert_eq!(status_on(&stdout), "aborted");
}

#[test]
fn a_run_that_could_not_start_prints_no_journal_of_an_earlier_run() {
    let lab = Lab::with_state("ok");
    let method = [inject(), heal()].concat();
    let earlier = run_for_json(&lab, &experiment(&method, &leave_marker()));
    assert_eq!(earlier.status.code(), Some(0));

    // The file of this run is no experiment. The journal of the earlier
    // run is still there, and `--force` lets a run write over it.
    let output = lab
        .run_command("title: no method\n")
        .args(["--output-format", "json", "--force"])
        .output()
        .expect("run tumult");

    assert_eq!(output.status.code(), Some(1));
    assert!(lab.path("journal.toon").exists());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}

#[test]
fn a_dry_run_has_no_journal_to_print_and_says_so() {
    let lab = Lab::with_state("ok");
    std::fs::write(
        lab.path("experiment.toon"),
        experiment(&inject(), &leave_marker()),
    )
    .expect("experiment");

    let output = lab.tumult(&[
        "run",
        "experiment.toon",
        "--dry-run",
        "--output-format",
        "json",
    ]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(2), "{stderr}");
    assert!(
        stderr.contains("--dry-run") && stderr.contains("cannot be used with"),
        "{stderr}"
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}
