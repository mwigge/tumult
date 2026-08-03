//! Tests for the HTML report renderer: every phase section, every activity
//! status (success/failure/timeout/skipped), analysis and regulatory blocks,
//! and the trace/detail corner cases.

use super::super::*;
use tumult_core::types::{
    ActivityResult, ActivityStatus, ActivityType, AnalysisResult, ExperimentStatus,
    HypothesisResult, Journal, RegulatoryMapping, SpanId, TraceId,
};

fn activity(
    name: &str,
    status: ActivityStatus,
    trace_id: &str,
    output: Option<&str>,
    error: Option<&str>,
) -> ActivityResult {
    ActivityResult {
        name: name.into(),
        activity_type: ActivityType::Action,
        status,
        started_at_ns: 1,
        duration_ms: 100,
        output: output.map(Into::into),
        error: error.map(Into::into),
        trace_id: TraceId(trace_id.into()),
        span_id: SpanId::empty(),
    }
}

/// A journal exercising every HTML section and status branch.
fn rich_journal() -> Journal {
    Journal {
        experiment_title: "rich report".into(),
        experiment_id: "rich-1".into(),
        status: ExperimentStatus::Deviated,
        started_at_ns: 1,
        ended_at_ns: 2,
        duration_ms: 60_000,
        steady_state_before: Some(HypothesisResult {
            title: "before".into(),
            met: true,
            probe_results: vec![activity(
                "pre-check",
                ActivityStatus::Succeeded,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                None,
                None,
            )],
        }),
        steady_state_after: Some(HypothesisResult {
            title: "after".into(),
            met: false,
            probe_results: vec![],
        }),
        method_results: vec![
            activity(
                "timed-out",
                ActivityStatus::Timeout,
                "",
                Some("partial output before timeout"),
                None,
            ),
            activity("skipped-step", ActivityStatus::Skipped, "", None, None),
            activity(
                "failed-no-detail",
                ActivityStatus::Failed,
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                None,
                None,
            ),
        ],
        rollback_results: vec![activity("undo", ActivityStatus::Succeeded, "", None, None)],
        rollback_failures: 0,
        estimate: None,
        baseline_result: None,
        during_result: None,
        post_result: None,
        load_result: None,
        analysis: Some(AnalysisResult {
            estimate_accuracy: Some(0.9),
            estimate_recovery_delta_s: None,
            trend: None,
            resilience_score: Some(0.75),
        }),
        regulatory: Some(RegulatoryMapping {
            frameworks: vec!["dora".into(), "soc2".into()],
            requirements: vec![],
        }),
        halt: None,
        blast_radius: None,
    }
}

#[test]
fn html_renders_every_phase_section_and_status() {
    let html = generate_html_report(&rich_journal(), None, "hash");

    // Phase sections.
    assert!(html.contains("Hypothesis Before: before (MET)"), "{html}");
    assert!(html.contains("Hypothesis After: after (NOT MET)"), "{html}");
    assert!(html.contains("Rollbacks"), "{html}");
    // Deviated status maps to the warning class.
    assert!(html.contains(r#"<span class="status warning">"#), "{html}");
    // Status glyphs: timeout hourglass, skipped dash, failure cross.
    assert!(html.contains("&#9203;"), "{html}");
    assert!(html.contains("&#8212;"), "{html}");
    assert!(html.contains("&#10008;"), "{html}");
    // Timed-out/failed rows are highlighted.
    assert!(html.contains(r#"<tr class="row-failed">"#), "{html}");
    // Failed activity with neither error nor output gets the fallback detail.
    assert!(html.contains("(no detail captured)"), "{html}");
}

#[test]
fn html_renders_analysis_and_regulatory_sections() {
    let html = generate_html_report(&rich_journal(), None, "hash");

    assert!(html.contains("<h2>Analysis</h2>"), "{html}");
    assert!(html.contains("90.0%"), "{html}");
    assert!(html.contains("0.75"), "{html}");
    assert!(html.contains("<h2>Regulatory Mapping</h2>"), "{html}");
    assert!(html.contains("dora, soc2"), "{html}");
}

#[test]
fn html_omits_optional_sections_when_absent() {
    use super::helpers::journal_with_failure;
    let html = generate_html_report(&journal_with_failure(), None, "hash");

    assert!(!html.contains("Hypothesis Before"), "{html}");
    assert!(!html.contains("Rollbacks"), "{html}");
    assert!(!html.contains("<h2>Analysis</h2>"), "{html}");
    assert!(!html.contains("<h2>Regulatory Mapping</h2>"), "{html}");
}

#[test]
fn html_activity_row_corner_cases() {
    // Empty trace id renders no trace cell content at all.
    let row = format_activity_row(
        &activity("no-trace", ActivityStatus::Succeeded, "", None, None),
        "method",
        Some("https://tempo.example"),
    );
    assert!(!row.contains("trace-link"), "{row}");

    // Failed activity with output but no error falls back to the output text.
    let row = format_activity_row(
        &activity(
            "out-only",
            ActivityStatus::Failed,
            "",
            Some("partial output"),
            None,
        ),
        "method",
        None,
    );
    assert!(row.contains("partial output"), "{row}");

    // A short trace id is not truncated below its length.
    let row = format_activity_row(
        &activity(
            "short-trace",
            ActivityStatus::Succeeded,
            "abc123",
            None,
            None,
        ),
        "method",
        None,
    );
    assert!(row.contains(">abc123</span>"), "{row}");
}
