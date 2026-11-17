//! The HTML report `tumult report` writes for the journal of a real run.
//!
//! The journals are the ones of runs in the lab (`sh` steps on a state
//! file) and one that the proof of the examples recorded. The report is
//! read from the file the binary wrote.
#![cfg(unix)]

mod lab;
mod report_page;

use std::path::{Path, PathBuf};

use lab::{experiment, heal, inject, leave_marker, step, step_within, with_block, Lab};
use report_page::{
    between, card, card_labels, chart_script, columns, failed_rows, has_section, report,
    report_command, report_of, row, rows, section, words, written, GENERATED_TEXT,
};
use tumult_core::types::{ExperimentStatus, HypothesisResult, Journal};
use tumult_test_utils::assert_kept;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace")
        .to_path_buf()
}

/// Where the first lines differ, for the message of a failed comparison.
fn first_difference(report: &str, kept: &str) -> String {
    let mut kept_lines = kept.lines();
    for (number, line) in report.lines().enumerate() {
        let kept_line = kept_lines.next().unwrap_or("(no further line)");
        if line != kept_line {
            return format!(
                "line {}:\n  report: {line}\n  kept:   {kept_line}",
                number + 1
            );
        }
    }
    "the kept report is longer".to_string()
}

#[test]
fn the_report_of_a_recorded_run_is_the_one_kept_with_the_tests() {
    let lab = Lab::with_state("ok");
    let recorded = workspace().join("examples/recorded/api-saturation.journal.toon");
    let kept_at = workspace().join("tumult-cli/tests/golden/api-saturation.report.html");
    let version = concat!("<strong>Tumult</strong> v", env!("CARGO_PKG_VERSION"));

    // The script of the charts is kept in a file of its own, and the report
    // of the next version is the same report.
    let html = report_of(&lab, &recorded)
        .replace(&chart_script(), "{report-charts.js}")
        .replace(version, "<strong>Tumult</strong> v{version}");

    assert_kept(&kept_at, &html);
}

#[test]
fn the_text_of_an_experiment_is_no_markup_in_its_report() {
    let lab = Lab::with_state("ok");
    let method = [
        step("show", "probe", "echo '<img src=x>'"),
        step("break", "action", "echo '<u>broken</u>' >&2; exit 3"),
    ]
    .concat();
    let definition = experiment(&method, &leave_marker())
        .replace(
            "title: Probe tolerance",
            r#"title: "<script>alert(1)</script> & co""#,
        )
        .replace("title: The target says ok", r#"title: "<b>steady</b>""#);
    let output = lab.run_command(&definition).output().expect("run tumult");
    let journal = lab.journal(&output);
    assert_eq!(journal.experiment_title, "<script>alert(1)</script> & co");

    // The run has steps, so its report has a chart and the script of the
    // charts, which is not text of the experiment.
    let html = report(&lab).replace(&chart_script(), "");

    assert!(
        html.contains(
            "<title>Tumult report: &lt;script&gt;alert(1)&lt;/script&gt; &amp; co</title>"
        ),
        "{html}"
    );
    assert!(
        html.contains("<h1>&lt;script&gt;alert(1)&lt;/script&gt; &amp; co</h1>"),
        "{html}"
    );
    assert!(html.contains("&lt;b&gt;steady&lt;/b&gt;"), "{html}");
    assert!(html.contains("&lt;img src=x&gt;"), "{html}");
    assert!(html.contains("&lt;u&gt;broken&lt;/u&gt;"), "{html}");
    for markup in ["<script>alert", "<b>steady", "<img", "<u>"] {
        assert!(!html.contains(markup), "{markup} in: {html}");
    }
}

/// Everything a definition can ask a run to judge and to write down.
const EVERY_BLOCK: &str = r#"kind: dr
target:
  system: lab-target
  technology: file
  environment: lab
  version: "1.4.2"

rpo_target_s: 5.0

nfr[1]{id,kind,metric,operator,threshold}:
  stays-up,availability,during.availability,gte,0.0

blast_radius: One state file in the lab
max_concurrent_faults: 2

estimate:
  expected_outcome: recovered
  expected_recovery_s: 2.0
  expected_degradation: moderate
  expected_data_loss: false
  confidence: high
  rationale: The heal step writes ok again
  prior_runs: 3

load:
  tool: oha
  script: "http://127.0.0.1:9/health"
  vus: 2
  duration_s: 2.0

regulatory:
  frameworks[2]: dora,nis2
  requirements[1]{id,description,evidence}:
    "DORA-25",Test of ICT tools and systems,The journal of this run
"#;

/// Run the experiment of every block in a new lab; the journal of the run
/// and its report.
fn report_of_every_block() -> (Journal, String) {
    let lab = Lab::with_state("ok");
    let method = [
        step("inject", "action", "echo down > state; sleep 1"),
        heal(),
    ]
    .concat();
    let definition = with_block(&experiment(&method, &leave_marker()), EVERY_BLOCK);
    let output = lab
        .run_command(&definition)
        .env("TUMULT_OHA_BINARY", lab.oha_stand_in())
        .output()
        .expect("run tumult");
    let journal = lab.journal(&output);
    assert_eq!(journal.status, ExperimentStatus::Completed);
    (journal, report(&lab))
}

#[test]
fn the_report_sums_the_run_up_in_its_cards() {
    let (_, html) = report_of_every_block();

    assert!(
        html.contains(r#"<span class="badge ok">completed</span>"#),
        "{html}"
    );
    assert_eq!(card(&html, "Kind"), "dr");
    assert_eq!(card(&html, "Target version"), "1.4.2 (declared)");
    assert_eq!(card(&html, "Steady state before"), "met");
    assert_eq!(card(&html, "Steady state after"), "met");
    assert_eq!(card(&html, "Objectives"), "2 of 3 met");
    assert_eq!(card(&html, "Rollbacks"), "none run");
    // A declared version is one version: there are no shares to show.
    assert!(!has_section(&html, "target-versions"), "{html}");
}

#[test]
fn the_report_shows_the_steady_state_and_the_steps_of_the_run() {
    let (journal, html) = report_of_every_block();

    let steady = section(&html, "steady-state");
    let titles: Vec<String> = steady
        .split("<h3>")
        .skip(1)
        .map(|check| words(check.split("</h3>").next().unwrap_or(check)))
        .collect();
    assert_eq!(
        titles,
        [
            "Before the method: The target says ok met",
            "After the method: The target says ok met"
        ]
    );
    assert_eq!(
        rows(steady),
        [
            row(&["Probe", "Status", "Duration", "Trace", "Output"]),
            row(&[
                "steady",
                "succeeded",
                &probe_time(journal.steady_state_before.as_ref()),
                "",
                "ok"
            ]),
            row(&["Probe", "Status", "Duration", "Trace", "Output"]),
            row(&[
                "steady",
                "succeeded",
                &probe_time(journal.steady_state_after.as_ref()),
                "",
                "ok"
            ]),
        ]
    );
    assert_eq!(
        columns(section(&html, "steps"), &[0, 1, 2, 3]),
        [
            row(&["Phase", "Step", "Type", "Status"]),
            row(&["method", "inject", "action", "succeeded"]),
            row(&["method", "heal", "action", "succeeded"]),
        ]
    );
}

/// How long the probe of `check` took, as a report writes it: the journal
/// has the milliseconds, and a probe of the lab takes less than a minute.
fn probe_time(check: Option<&HypothesisResult>) -> String {
    let check = check.expect("the journal has the check");
    let ms = check.probe_results[0].duration_ms;
    if ms < 1000 {
        return format!("{ms} ms");
    }
    format!(
        "{:.1} s",
        std::time::Duration::from_millis(ms).as_secs_f64()
    )
}

#[test]
fn the_report_shows_the_objectives_and_the_load_of_the_run() {
    let (_, html) = report_of_every_block();

    let objectives = section(&html, "objectives");
    assert_eq!(
        columns(objectives, &[0, 1, 2, 3, 6, 7]),
        [
            row(&["Recovery", "Target", "Measured"]),
            row(&["RTO", "30.0 s", "0 ms"]),
            row(&["RPO", "5.0 s", "not measured"]),
            row(&[
                "Objective",
                "Kind",
                "Metric",
                "Must be",
                "State",
                "Criticality"
            ]),
            row(&[
                "stays-up",
                "availability",
                "during.availability",
                "at least 0",
                "met",
                "major"
            ]),
            row(&[
                "rto",
                "rto",
                "recovery.mttr_s",
                "at most 30",
                "met",
                "blocking"
            ]),
            row(&["rpo", "rpo", "rpo", "at most 5", "inconclusive", "blocking"]),
        ]
    );
    assert!(
        rows(objectives).contains(&row(&[
            "rpo",
            "rpo",
            "rpo",
            "at most 5",
            "not measured",
            "",
            "inconclusive",
            "blocking"
        ])),
        "{objectives}"
    );
    let load = rows(section(&html, "load"));
    let stable: Vec<Vec<String>> = load
        .into_iter()
        .filter(|row| row[0] != "Duration")
        .map(|row| match row.len() {
            // A window: when it started is the time of this run.
            9 => row[1..].to_vec(),
            _ => row,
        })
        .collect();
    assert_eq!(
        stable,
        [
            row(&["Tool", "oha"]),
            row(&["Users", "2"]),
            row(&["Requests", "40"]),
            row(&["Throughput", "13.3 requests/s"]),
            row(&["Latency p50", "20.0 ms"]),
            row(&["Latency p95", "80.0 ms"]),
            row(&["Latency p99", "100.0 ms"]),
            row(&["Error rate", "0.00 %"]),
            row(&["Thresholds", "met"]),
            row(&["Windows", "1 of 1 completed"]),
            row(&["Requests by outcome", "sent 40, ok 40"]),
            row(&[
                "Length",
                "Users",
                "Requests",
                "Throughput",
                "p50",
                "p95",
                "p99",
                "Error rate"
            ]),
            row(&[
                "3.0 s",
                "2",
                "40",
                "13.3 requests/s",
                "20.0 ms",
                "80.0 ms",
                "100.0 ms",
                "0.00 %"
            ]),
        ]
    );
}

#[test]
fn the_report_shows_what_the_run_measured_and_what_it_was_planned_with() {
    let (_, html) = report_of_every_block();

    let measured = rows(section(&html, "measurements"));
    for fact in [
        row(&["Sample interval", "1.0 s"]),
        row(&["Full recovery", "yes"]),
        row(&["Recovery time", "0 ms"]),
    ] {
        assert!(measured.contains(&fact), "{fact:?} not in {measured:?}");
    }
    assert_eq!(
        rows(section(&html, "blast-radius")),
        [
            row(&["Note", "One state file in the lab"]),
            row(&["Cap on concurrent faults", "2"]),
            row(&["Peak concurrent faults", "0"]),
        ]
    );
    assert_eq!(
        rows(section(&html, "estimate")),
        [
            row(&["Expected outcome", "recovered"]),
            row(&["Expected recovery", "2.0 s"]),
            row(&["Expected degradation", "moderate"]),
            row(&["Expected data loss", "no"]),
            row(&["Confidence", "high"]),
            row(&["Rationale", "The heal step writes ok again"]),
            row(&["Prior runs", "3"]),
        ]
    );
    assert_eq!(
        rows(section(&html, "analysis")),
        [
            row(&["Estimate accuracy", "100.00 %"]),
            row(&["Resilience score", "1"]),
        ]
    );
    let regulatory = section(&html, "regulatory");
    assert!(
        words(regulatory).starts_with("Regulatory mapping Frameworks: dora, nis2 "),
        "{regulatory}"
    );
    assert_eq!(
        rows(regulatory),
        [
            row(&["Requirement", "Description", "Evidence"]),
            row(&[
                "DORA-25",
                "Test of ICT tools and systems",
                "The journal of this run"
            ]),
        ]
    );
}

#[test]
fn the_report_of_a_plain_run_has_no_section_for_what_the_run_did_not_have() {
    let lab = Lab::with_state("ok");
    let method = [inject(), heal()].concat();
    lab.run(&method);

    let html = report(&lab);

    for id in [
        "summary",
        "steady-state",
        "steps",
        "objectives",
        "measurements",
    ] {
        assert!(has_section(&html, id), "no {id}: {html}");
    }
    for id in [
        "halt",
        "load",
        "blast-radius",
        "estimate",
        "analysis",
        "regulatory",
    ] {
        assert!(!has_section(&html, id), "{id} in: {html}");
    }
    assert_eq!(
        card_labels(&html),
        [
            "Kind",
            "Started",
            "Duration",
            "Steady state before",
            "Steady state after",
            "Objectives",
            "Rollbacks"
        ]
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
fn the_report_of_a_halted_run_names_the_guard_and_says_that_the_load_was_cut() {
    let lab = Lab::with_state("ok");
    // The step would run for a minute. It breaks the guard at once.
    let method = step_within("inject", "action", "echo down > state; sleep 60", 90.0);
    let definition = with_block(&experiment(&method, &leave_marker()), GUARD_AND_LOAD);
    let output = lab
        .run_command(&definition)
        .env("TUMULT_OHA_BINARY", lab.oha_stand_in())
        .output()
        .expect("run tumult");
    let journal = lab.journal(&output);
    assert_eq!(journal.status, ExperimentStatus::Halted);
    let halt = journal.halt.expect("the journal has the halt");

    let html = report(&lab);

    assert!(
        html.contains(r#"<span class="badge fail">halted</span>"#),
        "{html}"
    );
    assert_eq!(card(&html, "Rollbacks"), "1 run, 0 failed");
    let halted = section(&html, "halt");
    assert!(words(halted).starts_with("Halted by a guard "), "{halted}");
    assert_eq!(
        rows(halted)[..4],
        [
            row(&["Guard", "state-guard"]),
            row(&["Safe condition", &halt.safe_condition]),
            row(&["Observed", "down"]),
            row(&["Breaches", "1"]),
        ]
    );
    let load = rows(section(&html, "load"));
    assert!(
        load.contains(&row(&["Cut", "the load was cut before its planned end"])),
        "{load:?}"
    );
    assert_eq!(
        columns(section(&html, "steps"), &[0, 1, 2, 3]),
        [
            row(&["Phase", "Step", "Type", "Status"]),
            row(&["method", "inject", "action", "failed"]),
            row(&["rollback", "leave-marker", "action", "succeeded"]),
        ]
    );
    assert_eq!(failed_rows(section(&html, "steps")), ["method"]);
    // The run was halted before it could look at the target again.
    assert_eq!(card(&html, "Steady state after"), "not checked");
    assert!(!has_section(&html, "objectives"), "{html}");
}

#[test]
fn the_report_of_a_run_that_deviated_shows_what_the_probe_read() {
    let lab = Lab::with_state("ok");
    // The fault is not healed: the target is down when the run checks it.
    let (_, journal) = lab.run(&inject());
    assert_eq!(journal.status, ExperimentStatus::Deviated);

    let html = report(&lab);

    assert!(
        html.contains(r#"<span class="badge warn">deviated</span>"#),
        "{html}"
    );
    assert_eq!(card(&html, "Steady state before"), "met");
    assert_eq!(card(&html, "Steady state after"), "not met");
    let steady = section(&html, "steady-state");
    assert!(
        words(steady).contains("After the method: The target says ok not met "),
        "{steady}"
    );
    let checks = columns(steady, &[0, 1, 4]);
    assert_eq!(
        checks,
        [
            row(&["Probe", "Status", "Output"]),
            row(&["steady", "succeeded", "ok"]),
            row(&["Probe", "Status", "Output"]),
            row(&[
                "steady",
                "failed",
                "down the output is outside the tolerance"
            ]),
        ]
    );
    assert_eq!(failed_rows(steady), ["steady"]);
}

#[test]
fn the_report_says_when_it_was_generated_and_can_be_built_again_to_the_byte() {
    let lab = Lab::with_state("ok");
    lab.run(&[inject(), heal()].concat());

    let first = report(&lab);
    let again = report(&lab);

    assert!(
        first.contains(&format!("Report generated: {GENERATED_TEXT}<br>")),
        "{first}"
    );
    assert!(first == again, "{}", first_difference(&first, &again));
}

#[test]
fn a_report_without_a_given_time_says_the_time_it_was_written() {
    let lab = Lab::with_state("ok");
    lab.run(&[inject(), heal()].concat());
    let before = chrono::Utc::now().timestamp();

    let output = report_command(&lab, &lab.path("journal.toon"))
        .env_remove("SOURCE_DATE_EPOCH")
        .output()
        .expect("run tumult");

    let html = written(&lab, &output);
    let generated = between(&html, "Report generated: ", "<br>");
    let at = chrono::DateTime::parse_from_rfc3339(generated)
        .unwrap_or_else(|e| panic!("{generated}: {e}"))
        .timestamp();
    assert!(
        (before..=before + 300).contains(&at),
        "generated at {generated}, asked for at {before}"
    );
}

#[test]
fn a_time_that_is_no_number_of_seconds_fails_the_report() {
    let lab = Lab::with_state("ok");
    lab.run(&[inject(), heal()].concat());

    let output = report_command(&lab, &lab.path("journal.toon"))
        .env("SOURCE_DATE_EPOCH", "tomorrow")
        .output()
        .expect("run tumult");

    assert!(!output.status.success());
    assert_eq!(
        lab::fatal_error(&output.stderr),
        "SOURCE_DATE_EPOCH is not a number of seconds: tomorrow"
    );
    assert!(!lab.path("report.html").exists(), "a report was written");
}
