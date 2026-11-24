//! What `tumult trend` and `tumult compliance` say about the journals of
//! real runs.
//!
//! Each test runs the binary against the lab, keeps the journals of its
//! runs in one directory, and reads what the binary prints about them.
#![cfg(unix)]

mod lab;

use lab::{count, experiment, heal, inject, leave_marker, step, with_block, Lab};
use serde_json::Value;

/// The directory, in the lab, of the journals of the runs.
const JOURNALS: &str = "journals";

/// The target block of a definition.
fn target(system: &str, technology: &str) -> String {
    format!("target:\n  system: {system}\n  technology: {technology}\n  environment: lab\n")
}

/// The experiment of the lab with this method, named `title`, on `target`.
fn on_target(title: &str, method: &str, target: &str) -> String {
    with_block(&experiment(method, &leave_marker()), target).replacen("Probe tolerance", title, 1)
}

/// An experiment with no steady state: its run has nothing to watch come
/// back.
fn without_steady_state() -> String {
    let method = step("fault", "action", "true");
    format!(
        "title: No steady state\ndescription: The run watches nothing\n\nmethod[{steps}]:\n{method}",
        steps = count(&method),
    )
}

/// Run `definition` and keep its journal as `journals/<name>.toon`. The
/// journal, as the JSON the run prints.
fn run(lab: &Lab, name: &str, definition: &str) -> Value {
    run_ending(lab, name, definition, 0)
}

/// Like [`run`], for a run that ends with the exit code `code`.
fn run_ending(lab: &Lab, name: &str, definition: &str, code: i32) -> Value {
    std::fs::create_dir_all(lab.path(JOURNALS)).expect("journals directory");
    let file = format!("{name}.toon");
    std::fs::write(lab.path(&file), definition).expect("experiment");
    let journal = format!("{JOURNALS}/{name}.toon");
    let output = lab.tumult(&[
        "run",
        &file,
        "--journal-path",
        &journal,
        "--no-ingest",
        "--output-format",
        "json",
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(code),
        "{name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(&stdout).unwrap_or_else(|e| panic!("stdout is not JSON ({e}):\n{stdout}"))
}

/// What `tumult <args>` prints.
fn printed(lab: &Lab, args: &[&str]) -> String {
    let output = lab.tumult(args);
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    stdout
}

#[test]
fn the_journal_of_a_run_names_the_target_of_its_definition() {
    let lab = Lab::with_state("ok");
    let method = [inject(), heal()].concat();

    let plain = run(&lab, "plain", &experiment(&method, &leave_marker()));
    let placed = run(
        &lab,
        "placed",
        &on_target("On a target", &method, &target("lab-target", "file")),
    );

    assert_eq!(plain.get("target"), None);
    assert_eq!(
        placed["target"],
        serde_json::json!({"system": "lab-target", "technology": "file", "environment": "lab"})
    );
}

#[test]
fn a_trend_by_target_is_over_the_runs_on_that_target_and_so_is_its_summary() {
    let lab = Lab::with_state("ok");
    let two_steps = [inject(), heal()].concat();
    let three_steps = [inject(), heal(), step("rest", "action", "true")].concat();
    run(&lab, "plain", &experiment(&two_steps, &leave_marker()));
    run(
        &lab,
        "placed",
        &on_target("On a target", &two_steps, &target("lab-target", "file")),
    );
    // The title of this run names the target of the other one.
    run(
        &lab,
        "elsewhere",
        &on_target(
            "Not on lab-target",
            &three_steps,
            &target("other-target", "disk"),
        ),
    );

    // By the system, in any case, or by the technology.
    for wanted in ["lab-target", "LAB-TARGET", "file"] {
        let trend = printed(
            &lab,
            &[
                "trend",
                JOURNALS,
                "--metric",
                "method_step_count",
                "--target",
                wanted,
            ],
        );

        assert!(
            trend.starts_with(
                "Loaded 3 journal(s)\nLeft out: 1 journal(s) that name no target (a journal \
                 names the target its experiment declares, since 3.0.2)\n\n\
                 Trend: method_step_count (1 data points)\n"
            ),
            "{wanted}: {trend}"
        );
        assert!(trend.contains("On a target"), "{wanted}: {trend}");
        assert!(!trend.contains("Not on lab-target"), "{wanted}: {trend}");
        assert!(
            trend.contains("\nSummary: 1 runs, min=2, max=2, "),
            "{wanted}: {trend}"
        );
    }

    let all = printed(&lab, &["trend", JOURNALS, "--metric", "method_step_count"]);
    assert!(
        all.starts_with("Loaded 3 journal(s)\n\nTrend: method_step_count (3 data points)\n"),
        "{all}"
    );
    assert!(all.contains("\nSummary: 3 runs, min=2, max=3, "), "{all}");
}

/// The block of a definition that watches 2 s for recovery, twice a second.
const SHORT_WATCH: &str = "recovery:\n  window_s: 2.0\n  interval_s: 0.5\n";

/// A DR test of the lab target named `title`, with an RTO of 30 s: the
/// experiment of the lab with this method and these rollbacks, watched for
/// 2 s after the method.
fn dr_test(title: &str, method: &str, rollbacks: &str) -> String {
    let definition = with_block(
        &experiment(method, rollbacks),
        &target("lab-target", "file"),
    );
    with_block(&definition, SHORT_WATCH).replacen("Probe tolerance", title, 1)
}

/// Run `definition` and keep its journal as `journals/<name>.toon`, also
/// when the run deviates. The exit code.
fn run_to_journal(lab: &Lab, name: &str, definition: &str) -> Option<i32> {
    std::fs::create_dir_all(lab.path(JOURNALS)).expect("journals directory");
    let file = format!("{name}.toon");
    std::fs::write(lab.path(&file), definition).expect("experiment");
    let journal = format!("{JOURNALS}/{name}.toon");
    let output = lab.tumult(&["run", &file, "--journal-path", &journal, "--no-ingest"]);
    assert!(
        lab.path(&journal).exists(),
        "{name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.status.code()
}

#[test]
fn a_trend_of_the_rto_of_real_runs_marks_the_run_that_did_not_recover() {
    let lab = Lab::with_state("ok");
    let hold = step("hold", "action", "sleep 1");
    let back = [inject(), hold, heal()].concat();
    let no_effect = step("inject", "action", "true");
    let codes = [
        run_to_journal(&lab, "a-back", &dr_test("Back", &back, &leave_marker())),
        run_to_journal(&lab, "b-never", &dr_test("Never back", &inject(), &heal())),
        run_to_journal(
            &lab,
            "c-no-outage",
            &dr_test("No outage", &no_effect, &leave_marker()),
        ),
    ];
    assert_eq!(codes, [Some(0), Some(1), Some(0)]);

    let args = [
        "trend",
        JOURNALS,
        "--metric",
        "rto_measured_s",
        "--target",
        "lab-target",
    ];
    let json = printed(&lab, &[&args[..], &["--format", "json"]].concat());
    let trend: Value = serde_json::from_str(&json).unwrap_or_else(|e| panic!("{e}: {json}"));

    let points = trend["points"].as_array().expect("points");
    let titles: Vec<&str> = points.iter().filter_map(|p| p["title"].as_str()).collect();
    assert_eq!(titles, ["Back", "Never back"], "{trend}");
    assert_eq!(points[0]["recovered"], true, "{trend}");
    assert_eq!(points[0].get("bound"), None, "{trend}");
    assert_eq!(points[1]["recovered"], false, "{trend}");
    assert_eq!(points[1]["bound"], "at least", "{trend}");
    // The RTO the first run measured, and the time the second one watched.
    let objectives = |name: &str| {
        let path = lab.path(&format!("{JOURNALS}/{name}.toon"));
        let journal = tumult_core::journal::read_journal(&path).expect("journal");
        journal.objectives.expect("objectives")
    };
    let back = objectives("a-back").rto_measured_s.expect("an RTO");
    let never = objectives("b-never");
    assert_eq!(never.rto_measured_s, None);
    let watched = never.verdicts[0].observed.expect("the time watched");
    assert_eq!(points[0]["value"].as_f64(), Some(back), "{trend}");
    assert_eq!(points[1]["value"].as_f64(), Some(watched), "{trend}");
    assert_eq!(trend["summary"]["runs"], 1, "{trend}");
    assert_eq!(trend["runs_without_outage"], 1, "{trend}");

    let text = printed(&lab, &args);
    let never = text
        .lines()
        .find(|line| line.contains("Never back"))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(never.contains(" at least "), "{text}");
    assert!(never.trim_end().ends_with("not recovered"), "{text}");
    assert!(text.ends_with("\n1 run(s) saw no outage\n"), "{text}");
}

#[test]
fn compliance_counts_recovery_from_the_runs_that_watched_for_it() {
    let lab = Lab::with_state("ok");
    run(&lab, "unwatched", &without_steady_state());

    // The run completed. That is its pass, and no sign of a recovery.
    let report = printed(&lab, &["compliance", JOURNALS, "--framework", "dora"]);
    assert!(
        report.contains(
            "  Pass rate: 100.0%\n  \
             Recovery within target: not measured, no run measured a recovery. \
             The verdict rests on the pass rate only (reduced assurance).\n  \
             Evidence verdict: COMPLIANT (pass-rate only)\n"
        ),
        "{report}"
    );

    // The fault holds past one read of the probes, so the run sees the
    // outage and measures its recovery.
    let method = [inject(), step("hold", "action", "sleep 1.5"), heal()].concat();
    run(&lab, "watched", &experiment(&method, &leave_marker()));

    let report = printed(&lab, &["compliance", JOURNALS, "--framework", "dora"]);
    assert!(
        report.contains(
            "  Pass rate: 100.0%\n  \
             Recovery within target: 100.0% of the runs that measured a recovery \
             (1 of 1; 1 against their own RTO, 0 against 30s)\n  \
             Evidence verdict: COMPLIANT\n"
        ),
        "{report}"
    );
}

#[test]
fn compliance_judges_each_run_against_its_own_rto_and_says_when_none_measured_a_recovery() {
    let lab = Lab::with_state("ok");
    // The fault has no visible effect: the run watched and measured no
    // recovery.
    let still = step("fault", "action", "true");
    run(&lab, "still", &experiment(&still, &leave_marker()));

    let output = lab.tumult(&["compliance", JOURNALS, "--framework", "dora"]);
    let report = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{report}");
    assert!(
        report.contains(
            "  Pass rate: 100.0%\n  \
             Recovery within target: not measured, no run measured a recovery. \
             The verdict rests on the pass rate only (reduced assurance).\n  \
             Evidence verdict: COMPLIANT (pass-rate only)\n"
        ),
        "{report}"
    );

    // The target is down for about two seconds after the method. That is
    // within the 30 s of a run with no RTO, and outside the RTO of one
    // second this run declares, so the run deviates.
    let slow = step(
        "fault",
        "action",
        "echo down > state; (sleep 2; echo ok > state) >/dev/null 2>&1 &",
    );
    let tight =
        experiment(&slow, &leave_marker()).replacen("rto_target_s: 30.0", "rto_target_s: 1.0", 1);
    run_ending(&lab, "tight", &tight, 1);

    let output = lab.tumult(&["compliance", JOURNALS, "--framework", "dora"]);
    let report = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{report}");
    assert!(
        report.contains(
            "  Pass rate: 50.0%\n  \
             Recovery within target: 0.0% of the runs that measured a recovery \
             (0 of 1; 1 against their own RTO, 0 against 30s). \
             Left out: 1 run(s) that measured no recovery.\n  \
             Evidence verdict: NON-COMPLIANT\n"
        ),
        "{report}"
    );
}
