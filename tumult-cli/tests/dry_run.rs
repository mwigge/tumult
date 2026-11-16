//! What `tumult run --dry-run` prints, and that it runs nothing.
//!
//! The binary reads a real experiment file in a lab whose state file would
//! say `down` after the first step of the method.
#![cfg(unix)]

mod lab;

use lab::{experiment, heal, inject, leave_marker, with_block, Lab};

/// What the plan is judged by, and a guard on it.
const JUDGED_BY: &str = r#"kind: dr
target:
  system: lab-target
  technology: file
  environment: lab

nfr[1]{id,kind,metric,operator,threshold}:
  fast,latency,probe.steady.p95_ms,lte,100

load:
  tool: oha
  script: "http://127.0.0.1:9/health"
  vus: 2
  duration_s: 3.0

guards[1]:
  - name: watch
    probe:
      name: watch-read
      activity_type: probe
      provider:
        type: process
        path: cat
        arguments[1]: state
      tolerance:
        type: regex
        pattern: "^ok$"
    min_breaches: 2
"#;

const PLAN: &str = "=== DRY RUN ===

Experiment: Probe tolerance
Description: The fault writes down to a state file and the probes read it
Kind: dr
Target: lab-target (technology: file, environment: lab)
Recovery targets: RTO 30s

Objectives (1):
  - fast (latency, major): probe.steady.p95_ms lte 100

Load: oha http://127.0.0.1:9/health (2 users, 3s, flat)

Hypothesis: The target says ok
  Probe: steady

Guards (1):
  - watch: probe watch-read, halts after 2 breaches

Phase 2 — Method (2 steps):
  1. inject (Action)
  2. heal (Action)

Rollbacks (1 steps):
  - leave-marker (Action)

=== END DRY RUN ===
";

/// Dry-run `definition` in `lab`; whether it passed and what it printed.
fn dry_run(lab: &Lab, definition: &str) -> (bool, String) {
    std::fs::write(lab.path("experiment.toon"), definition).expect("experiment");
    let output = lab.tumult(&["run", "experiment.toon", "--dry-run"]);
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    )
}

#[test]
fn a_dry_run_prints_the_plan_and_what_it_is_judged_by() {
    let lab = Lab::with_state("ok");
    let method = [inject(), heal()].concat();
    let definition = with_block(&experiment(&method, &leave_marker()), JUDGED_BY);

    let (passed, stdout) = dry_run(&lab, &definition);

    assert!(passed, "{stdout}");
    assert_eq!(stdout, PLAN);
}

/// The `Load:` line of the dry run of a load with `profile` as its shape.
fn load_line(profile: &str) -> String {
    let lab = Lab::with_state("ok");
    let load = format!(
        "load:\n  tool: oha\n  script: \"http://127.0.0.1:9/health\"\n  vus: 2\n  \
         duration_s: 6.0\n{profile}"
    );
    let definition = with_block(&experiment(&inject(), &leave_marker()), &load);

    let (passed, stdout) = dry_run(&lab, &definition);

    assert!(passed, "{stdout}");
    let line = stdout.lines().find(|line| line.starts_with("Load:"));
    line.unwrap_or_default().to_string()
}

#[test]
fn a_dry_run_names_the_shape_of_the_load() {
    let shapes = [
        ("", "flat"),
        (
            "  profile:\n    type: ramp\n    start_vus: 1\n    ramp_up_s: 2.0\n",
            "ramp",
        ),
        (
            "  profile:\n    type: spike\n    peak_vus: 4\n    after_s: 2.0\n    hold_s: 2.0\n",
            "spike",
        ),
        (
            "  profile:\n    type: soak\n    sample_interval_s: 2.0\n",
            "soak",
        ),
    ];
    for (profile, shape) in shapes {
        assert_eq!(
            load_line(profile),
            format!("Load: oha http://127.0.0.1:9/health (2 users, 6s, {shape})")
        );
    }
}

#[test]
fn a_dry_run_runs_no_step_and_leaves_no_journal_and_no_store() {
    let lab = Lab::with_state("ok");
    // A run of this method leaves the target down.
    let definition = experiment(&inject(), &leave_marker());

    let (passed, stdout) = dry_run(&lab, &definition);

    assert!(passed, "{stdout}");
    let state = std::fs::read_to_string(lab.path("state")).expect("state file");
    assert_eq!(state, "ok\n", "the fault ran");
    assert!(!lab.rolled_back(), "the rollback ran");
    let mut left: Vec<String> = std::fs::read_dir(lab.path(""))
        .expect("lab directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    left.sort();
    assert_eq!(left, ["experiment.toon", "state"]);
}

#[test]
fn a_dry_run_of_a_file_that_is_not_valid_fails_as_a_run_does() {
    let lab = Lab::with_state("ok");
    let definition = with_block(
        &experiment(&inject(), &leave_marker()),
        "nfr[1]{id,kind,metric,operator,threshold}:\n  fast,latency,latency.p42,lte,100\n",
    );

    let (passed, stdout) = dry_run(&lab, &definition);

    assert!(!passed, "{stdout}");
    assert_eq!(stdout, "");
}
