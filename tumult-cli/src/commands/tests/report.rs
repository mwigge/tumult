//! Tests for `cmd_report` (html/junit/json) and compliance reporting.

use super::super::*;
use super::helpers::*;
use tempfile::TempDir;
use tumult_core::execution::RollbackStrategy;

// ── Phase 3: Report command ──────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_generates_html_file() {
    let d = TempDir::new().unwrap();
    let exp_path = write_valid_experiment(d.path());
    let journal_path = d.path().join("journal.toon");

    cmd_run(
        &exp_path,
        &journal_path,
        false,
        false,
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await
    .unwrap();

    let report_path = d.path().join("report.html");
    cmd_report(&journal_path, Some(&report_path), ReportFormat::Html, None).unwrap();
    assert!(report_path.exists());

    let content = std::fs::read_to_string(&report_path).unwrap();
    assert!(content.contains("<!DOCTYPE html>"));
    assert!(content.contains("Tumult Experiment Report"));
    assert!(content.contains("CLI test experiment"));
    assert!(content.contains("Activity Timeline"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_default_output_uses_journal_stem() {
    let d = TempDir::new().unwrap();
    let exp_path = write_valid_experiment(d.path());
    let journal_path = d.path().join("my-experiment.toon");

    cmd_run(
        &exp_path,
        &journal_path,
        false,
        false,
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await
    .unwrap();

    // Change to temp dir so default output lands there
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(d.path()).unwrap();
    cmd_report(&journal_path, None, ReportFormat::Html, None).unwrap();
    std::env::set_current_dir(prev).unwrap();

    assert!(d.path().join("my-experiment.html").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_html_contains_trace_ids() {
    let d = TempDir::new().unwrap();
    let exp_path = write_valid_experiment(d.path());
    let journal_path = d.path().join("journal.toon");

    cmd_run(
        &exp_path,
        &journal_path,
        false,
        false,
        RollbackStrategy::OnDeviation,
        false,
        std::collections::HashMap::new(),
        None,
    )
    .await
    .unwrap();

    let report_path = d.path().join("report.html");
    cmd_report(&journal_path, Some(&report_path), ReportFormat::Html, None).unwrap();

    let content = std::fs::read_to_string(&report_path).unwrap();
    // Should contain method steps
    assert!(content.contains("echo-action"));
}

#[test]
fn report_nonexistent_journal_returns_error() {
    let result = cmd_report(
        Path::new("/nonexistent.toon"),
        None,
        ReportFormat::Html,
        None,
    );
    assert!(result.is_err());
}

// ── Report: junit / json / trace-links / detail / footer ──

#[test]
fn junit_report_has_testsuite_and_failure() {
    let journal = journal_with_failure();
    let xml = generate_junit_report(&journal);
    assert!(xml.contains(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
    assert!(xml.contains(r#"<testsuite name="Failure &amp; &lt;recovery&gt;""#));
    assert!(xml.contains(r#"tests="2""#));
    assert!(xml.contains(r#"failures="1""#));
    assert!(xml.contains(r#"<testcase name="ok-step" classname="method""#));
    assert!(xml.contains(r#"<testcase name="bad-step" classname="method""#));
    // Failed activity surfaces a <failure> element carrying the error text.
    assert!(xml.contains("<failure"));
    assert!(xml.contains("connection refused on port 5432"));
    assert!(xml.trim_end().ends_with("</testsuite>"));
}

#[test]
fn junit_report_written_via_cmd_report() {
    use tumult_core::journal::write_journal;
    let d = TempDir::new().unwrap();
    let journal_path = d.path().join("j.toon");
    write_journal(&journal_with_failure(), &journal_path).unwrap();
    let out = d.path().join("report.xml");
    cmd_report(&journal_path, Some(&out), ReportFormat::Junit, None).unwrap();
    let xml = std::fs::read_to_string(&out).unwrap();
    assert!(xml.contains("<testsuite"));
    assert!(xml.contains(r#"failures="1""#));
}

#[test]
fn json_report_is_valid_and_roundtrips() {
    use tumult_core::journal::write_journal;
    let d = TempDir::new().unwrap();
    let journal_path = d.path().join("j.toon");
    let original = journal_with_failure();
    write_journal(&original, &journal_path).unwrap();
    let out = d.path().join("report.json");
    cmd_report(&journal_path, Some(&out), ReportFormat::Json, None).unwrap();
    let json = std::fs::read_to_string(&out).unwrap();
    // Round-trips back into a Journal equal to the source.
    let parsed: tumult_core::types::Journal = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, original);
    assert!(json.contains("\"experiment_id\""));
}

#[test]
fn html_trace_ui_base_renders_clickable_full_id_link() {
    let journal = journal_with_failure();
    let html = generate_html_report(&journal, Some("https://tempo.example/"), "deadbeef");
    // Uses the FULL trace_id in the href (not the 16-char display truncation),
    // and the trailing slash on the base is trimmed.
    assert!(html.contains(
        r#"<a class="trace-link" href="https://tempo.example/trace/aabbccddeeff00112233445566778899">"#
    ));
    // Display text is still truncated to 16 chars.
    assert!(html.contains(">aabbccddeeff0011</a>"));
}

#[test]
fn html_without_trace_ui_base_is_plain_text() {
    let journal = journal_with_failure();
    let html = generate_html_report(&journal, None, "deadbeef");
    assert!(!html.contains("<a class=\"trace-link\""));
    assert!(html.contains(r#"<span class="trace-link">aabbccddeeff0011</span>"#));
}

#[test]
fn html_surfaces_failure_detail() {
    let journal = journal_with_failure();
    let html = generate_html_report(&journal, None, "deadbeef");
    assert!(html.contains(r#"<span class="detail-error">connection refused on port 5432</span>"#));
    // No fabricated diagnostic command is emitted (the core path has no such field).
    assert!(!html.contains("tumult replay --trace"));
    // Failed rows carry the row-failed class.
    assert!(html.contains(r#"<tr class="row-failed">"#));
}

#[test]
fn html_footer_has_version_and_hash() {
    let journal = journal_with_failure();
    let html = generate_html_report(&journal, None, "0123abcd0123abcd");
    assert!(html.contains(concat!("Tumult</strong> v", env!("CARGO_PKG_VERSION"))));
    assert!(html.contains("Report generated:"));
    assert!(html.contains("<code>0123abcd0123abcd</code>"));
}

// ── Compliance: recovery-aware verdict ────────────────────

#[test]
fn compliance_verdict_requires_both_pass_and_recovery() {
    // High pass rate but weak recovery cannot be COMPLIANT.
    assert_eq!(compliance_verdict(1.0, Some(0.80)), "PARTIAL");
    assert_eq!(compliance_verdict(1.0, Some(0.50)), "NON-COMPLIANT");
    // Both strong.
    assert_eq!(compliance_verdict(0.96, Some(0.95)), "COMPLIANT");
    // Partial band.
    assert_eq!(compliance_verdict(0.85, Some(0.80)), "PARTIAL");
    // Strong recovery but weak pass rate is not COMPLIANT.
    assert_eq!(compliance_verdict(0.70, Some(0.99)), "NON-COMPLIANT");
}

#[test]
fn compliance_verdict_pass_rate_only_fallback() {
    assert_eq!(compliance_verdict(0.96, None), "COMPLIANT (pass-rate only)");
    assert_eq!(compliance_verdict(0.85, None), "PARTIAL (pass-rate only)");
    assert_eq!(compliance_verdict(0.50, None), "NON-COMPLIANT");
}

#[test]
fn compliance_runs_over_journal_with_mttr() {
    use tumult_core::journal::write_journal;
    use tumult_core::types::*;

    let d = TempDir::new().unwrap();
    let journal_path = d.path().join("compliant.toon");
    let mut journal = journal_with_failure();
    journal.status = ExperimentStatus::Completed;
    journal.post_result = Some(PostResult {
        started_at_ns: 1_774_980_002_000_000_000,
        ended_at_ns: 1_774_980_010_000_000_000,
        duration_s: 8.0,
        samples: 4,
        probes: vec![],
        recovery_time_s: 5.0,
        full_recovery: true,
        residual_degradation: None,
        data_integrity_verified: None,
        data_loss_detected: None,
        mttr_s: Some(5.0),
    });
    write_journal(&journal, &journal_path).unwrap();

    // Exercises the MTTR-based recovery accumulation path end to end.
    cmd_compliance(Some(&journal_path), ComplianceFramework::Soc2, false).unwrap();
    // Exercises the --sources registry-listing path (no journals needed).
    cmd_compliance(None, ComplianceFramework::Soc2, true).unwrap();
}
