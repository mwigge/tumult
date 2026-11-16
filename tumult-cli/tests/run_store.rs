//! What `tumult run` leaves in the analytics store.
//!
//! The runs here are real: the binary runs `sh` steps on a state file and
//! writes each journal to a private store. The store is then read with
//! `tumult analyze`, as a user reads it.
#![cfg(unix)]

mod lab;

use lab::{experiment, heal, inject, leave_marker, reads, step, Lab};

/// Run `method` as the experiment `name` and store its journal; what the
/// binary printed.
fn run_into_the_store(lab: &Lab, name: &str, method: &str) -> String {
    let definition = format!("{name}.toon");
    std::fs::write(lab.path(&definition), experiment(method, &leave_marker())).expect("experiment");
    let journal = format!("{name}-journal.toon");
    let output = lab.tumult(&["run", &definition, "--journal-path", &journal]);
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The rows `tumult analyze --query` prints for `sql`, cell by cell.
fn rows(lab: &Lab, sql: &str) -> Vec<Vec<String>> {
    let output = lab.tumult(&["analyze", "--query", sql]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
        .lines()
        .skip_while(|line| !line.starts_with("---"))
        .skip(1)
        .take_while(|line| !line.is_empty())
        .map(|line| line.split('\t').map(str::to_string).collect())
        .collect()
}

#[test]
fn the_store_holds_every_run_as_it_ended() {
    let lab = Lab::with_state("ok");
    let runs = [
        ("stays-up", [inject(), heal()].concat()),
        (
            "probe-fails",
            [inject(), reads("during", "ok"), heal()].concat(),
        ),
        ("fault-fails", step("inject", "action", "exit 3")),
        ("down-before", inject()),
    ];

    for (name, method) in &runs {
        // The last run finds its target down before the fault.
        let state = if *name == "down-before" { "down" } else { "ok" };
        std::fs::write(lab.path("state"), format!("{state}\n")).expect("state file");
        let stdout = run_into_the_store(&lab, name, method);
        assert!(
            stdout.contains("Ingested into persistent analytics store\n"),
            "{name}: {stdout}"
        );
    }

    let roots = rows(
        &lab,
        "SELECT outcome_status, hypothesis_met, journal_only, trace_id \
         FROM run_roots ORDER BY outcome_status",
    );
    assert_eq!(
        roots,
        [
            ["aborted", "false", "true", "NULL"],
            ["completed", "true", "true", "NULL"],
            ["deviated", "true", "true", "NULL"],
            ["failed", "true", "true", "NULL"],
        ]
    );
    let steps = rows(
        &lab,
        "SELECT name, status, phase FROM activity_results ORDER BY started_at_ns",
    );
    assert_eq!(
        steps,
        [
            // The target stays up.
            ["steady", "succeeded", "hypothesis_before"],
            ["inject", "succeeded", "method"],
            ["heal", "succeeded", "method"],
            ["steady", "succeeded", "hypothesis_after"],
            // A probe of the method does not hold.
            ["steady", "succeeded", "hypothesis_before"],
            ["inject", "succeeded", "method"],
            ["during", "failed", "method"],
            ["heal", "succeeded", "method"],
            ["steady", "succeeded", "hypothesis_after"],
            ["leave-marker", "succeeded", "rollback"],
            // The fault itself fails.
            ["steady", "succeeded", "hypothesis_before"],
            ["inject", "failed", "method"],
            ["steady", "succeeded", "hypothesis_after"],
            ["leave-marker", "succeeded", "rollback"],
            // The target is down before the fault.
            ["steady", "failed", "hypothesis_before"],
            ["leave-marker", "succeeded", "rollback"],
        ]
    );
}
