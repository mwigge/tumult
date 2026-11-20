//! The reports `tumult report` writes for the journal of a real run.
//!
//! The runs are the ones of the lab: `sh` steps on a state file. The reports
//! are read from the files the binary wrote.
#![cfg(unix)]

mod lab;

use std::path::Path;

use lab::{experiment, heal, inject, leave_marker, Lab};
use sha2::{Digest, Sha256};
use tumult_test_utils::assert_kept;

const STEADY: &str = "The target says ok";

/// Write the report of `journal.toon` in `format` and read it.
fn report(lab: &Lab, format: &str) -> String {
    report_of(lab, "journal.toon", format)
}

/// Write the report of the journal at `journal` in `format` and read it.
fn report_of(lab: &Lab, journal: &str, format: &str) -> String {
    let file = format!("report.{format}");
    let output = lab.tumult(&["report", journal, "--format", format, "--output", &file]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::read_to_string(lab.path(&file)).expect("report")
}

/// The value of the attribute `name` of an XML element.
fn attribute(element: &str, name: &str) -> String {
    let start = format!(r#"{name}=""#);
    let Some((value, _)) = element
        .split_once(&start)
        .and_then(|(_, rest)| rest.split_once('"'))
    else {
        panic!("no {name}: {element}");
    };
    value.to_string()
}

/// The cases of a `JUnit` report: name, class and the failure, if any.
fn cases(xml: &str) -> Vec<(String, String, Option<String>)> {
    xml.split("<testcase ")
        .skip(1)
        .map(|case| {
            let failure = case.split_once("<failure ").map(|(_, failure)| {
                let Some((text, _)) = failure
                    .split_once('>')
                    .and_then(|(_, rest)| rest.split_once("</failure>"))
                else {
                    panic!("no failure text: {case}");
                };
                format!("{}: {text}", attribute(failure, "message"))
            });
            (
                attribute(case, "name"),
                attribute(case, "classname"),
                failure,
            )
        })
        .collect()
}

fn case(name: &str, class: &str, failure: Option<&str>) -> (String, String, Option<String>) {
    (name.into(), class.into(), failure.map(str::to_string))
}

#[test]
fn the_junit_report_has_a_case_for_each_check_of_the_steady_state() {
    let lab = Lab::with_state("ok");
    let method = [inject(), heal()].concat();
    lab.run(&method);

    let xml = report(&lab, "junit");

    assert_eq!(
        cases(&xml),
        [
            case(STEADY, "hypothesis_before", None),
            case("steady", "hypothesis_before", None),
            case("inject", "method", None),
            case("heal", "method", None),
            case(STEADY, "hypothesis_after", None),
            case("steady", "hypothesis_after", None),
        ]
    );
    let suite = xml.split("<testsuite ").nth(1).expect("a suite");
    assert_eq!(attribute(suite, "tests"), "6");
    assert_eq!(attribute(suite, "failures"), "0");
}

#[test]
fn a_steady_state_that_did_not_hold_is_a_failed_case_of_its_own() {
    // The target is down before the fault, so the run stops at the first
    // check and only rolls back.
    let lab = Lab::with_state("down");
    lab.run(&inject());

    let xml = report(&lab, "junit");

    assert_eq!(
        cases(&xml),
        [
            case(
                STEADY,
                "hypothesis_before",
                Some("NotMet: the steady state did not hold")
            ),
            case(
                "steady",
                "hypothesis_before",
                Some("Failed: the output is outside the tolerance")
            ),
            case("leave-marker", "rollback", None),
        ]
    );
    let suite = xml.split("<testsuite ").nth(1).expect("a suite");
    assert_eq!(attribute(suite, "tests"), "3");
    assert_eq!(attribute(suite, "failures"), "2");
}

#[test]
fn the_junit_report_of_a_recorded_run_is_the_one_kept_with_the_tests() {
    let lab = Lab::with_state("ok");
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace");
    let recorded = workspace.join("examples/recorded/api-saturation.journal.toon");
    let kept_at = workspace.join("tumult-cli/tests/golden/api-saturation.report.junit.xml");

    let xml = report_of(&lab, recorded.to_str().expect("a path"), "junit");

    assert_kept(&kept_at, &xml);
}

#[test]
fn the_run_and_the_html_report_name_the_sha_256_of_the_journal_file() {
    let lab = Lab::with_state("ok");
    let method = [inject(), heal()].concat();
    let output = lab
        .run_command(&experiment(&method, &leave_marker()))
        .output()
        .expect("run tumult");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let journal = std::fs::read(lab.path("journal.toon")).expect("journal");
    let of_the_file: String = Sha256::digest(&journal)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<String>>()
        .concat();

    let html = report(&lab, "html");

    let printed = stdout
        .lines()
        .find_map(|line| line.strip_prefix("Journal SHA-256: "))
        .unwrap_or_else(|| panic!("the run printed no hash: {stdout}"));
    assert_eq!(printed, of_the_file);
    assert!(
        html.contains(&format!(
            "Source journal SHA-256: <code>{of_the_file}</code>"
        )),
        "{html}"
    );
}
