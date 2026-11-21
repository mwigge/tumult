//! The corner cases of a step row of the HTML report, and the time a
//! report says it was generated at.
//!
//! What a whole report holds is tested through the binary, in
//! `tests/report_html.rs` and `tests/report_journals.rs`.

use super::super::*;
use chrono::{DateTime, Utc};
use tumult_core::types::{ActivityResult, ActivityStatus, ActivityType, SpanId, TraceId};

/// 2026-11-16 22:15:19 UTC.
const RUN_STARTED: i64 = 1_794_867_319_000_000_000;

/// A step that began a second and a half into the run.
fn step(
    status: ActivityStatus,
    trace_id: &str,
    output: Option<&str>,
    error: Option<&str>,
) -> ActivityResult {
    ActivityResult {
        name: "a-step".into(),
        activity_type: ActivityType::Action,
        status,
        started_at_ns: RUN_STARTED + 1_500_000_000,
        duration_ms: 100,
        output: output.map(Into::into),
        error: error.map(Into::into),
        trace_id: TraceId(trace_id.into()),
        span_id: SpanId::empty(),
    }
}

/// The row of a step of the method, cell by cell.
fn cells(step: &ActivityResult, trace_ui_base: Option<&str>) -> Vec<String> {
    let row = step_row("method", step, RUN_STARTED, trace_ui_base);
    let row = row
        .strip_suffix("</td></tr>\n")
        .unwrap_or_else(|| panic!("no end of a row in: {row}"));
    let (_, cells) = row
        .split_once("><td>")
        .unwrap_or_else(|| panic!("no first cell in: {row}"));
    cells.split("</td><td>").map(str::to_string).collect()
}

#[test]
fn a_step_that_went_well_and_said_nothing_is_a_plain_row() {
    let quiet = step(ActivityStatus::Succeeded, "", None, None);

    let row = step_row("method", &quiet, RUN_STARTED, Some("https://tempo.example"));

    assert_eq!(
        row,
        "<tr><td>method</td><td>a-step</td><td>action</td>\
         <td><span class=\"badge ok\">succeeded</span></td>\
         <td>+1.5 s</td><td>100 ms</td><td></td><td></td></tr>\n"
    );
}

#[test]
fn a_step_that_failed_is_a_marked_row_with_what_it_said() {
    let failed = step(ActivityStatus::Failed, "", Some("partial output"), None);

    let row = step_row("rollback", &failed, RUN_STARTED, None);

    assert_eq!(
        row,
        "<tr class=\"row-failed\"><td>rollback</td><td>a-step</td><td>action</td>\
         <td><span class=\"badge fail\">failed</span></td>\
         <td>+1.5 s</td><td>100 ms</td><td></td>\
         <td><span class=\"detail-output\">partial output</span></td></tr>\n"
    );
}

#[test]
fn words_that_are_empty_are_no_words() {
    let failed = step(ActivityStatus::Failed, "", Some(""), Some(""));
    let went_well = step(ActivityStatus::Succeeded, "", Some(""), Some(""));

    assert_eq!(
        cells(&failed, None)[7],
        r#"<span class="detail-error">(no detail captured)</span>"#
    );
    assert_eq!(cells(&went_well, None)[7], "");
}

#[test]
fn a_trace_is_named_by_its_first_sixteen_characters() {
    let full = "aabbccddeeff00112233445566778899";
    let long = step(ActivityStatus::Succeeded, full, None, None);
    let short = step(ActivityStatus::Succeeded, "abc123", None, None);

    assert_eq!(
        cells(&long, None)[6],
        r#"<span class="trace-link">aabbccddeeff0011</span>"#
    );
    assert_eq!(
        cells(&short, None)[6],
        r#"<span class="trace-link">abc123</span>"#
    );
}

#[test]
fn a_trace_links_to_the_whole_trace_in_the_trace_ui() {
    let full = "aabbccddeeff00112233445566778899";
    let traced = step(ActivityStatus::Succeeded, full, None, None);

    for base in ["https://tempo.example", "https://tempo.example//"] {
        assert_eq!(
            cells(&traced, Some(base))[6],
            format!(
                r#"<a class="trace-link" href="https://tempo.example/trace/{full}">aabbccddeeff0011</a>"#
            ),
            "{base}"
        );
    }
}

#[test]
fn a_step_from_before_the_run_began_has_a_start_below_zero() {
    let early = ActivityResult {
        started_at_ns: RUN_STARTED - 250_000_000,
        ..step(ActivityStatus::Succeeded, "", None, None)
    };
    let at_once = ActivityResult {
        started_at_ns: RUN_STARTED,
        ..step(ActivityStatus::Succeeded, "", None, None)
    };

    assert_eq!(cells(&early, None)[4], "-250 ms");
    assert_eq!(cells(&at_once, None)[4], "+0 ms");
}

#[test]
fn a_report_is_generated_at_the_time_it_is_told() {
    let at = generated_at(Some("1794873600")).unwrap();

    assert_eq!(at.to_rfc3339(), "2026-11-17T00:00:00+00:00");
}

#[test]
fn a_report_that_is_told_no_time_is_generated_now() {
    let before = Utc::now();

    let at = generated_at(None).unwrap();

    assert!(before <= at && at <= Utc::now(), "{at}");
}

#[test]
fn a_time_that_is_no_number_of_seconds_is_an_error() {
    for given in ["tomorrow", "", "1794873600.5", "9223372036854775807"] {
        let error = generated_at(Some(given)).unwrap_err();

        assert_eq!(
            error.to_string(),
            format!("SOURCE_DATE_EPOCH is not a number of seconds: {given}")
        );
    }
}

#[test]
fn the_first_second_of_the_epoch_is_a_time() {
    assert_eq!(generated_at(Some("0")).unwrap(), DateTime::UNIX_EPOCH);
}
