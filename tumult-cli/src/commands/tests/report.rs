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
    assert!(content.contains(r#"<p class="eyebrow">Tumult run report</p>"#));
    assert!(content.contains("<h1>CLI test experiment</h1>"));
    assert!(content.contains(r#"<section id="steps">"#));
}

// The cwd guard must cover the awaited report call (the cwd is read inside
// it). Only tests serialized on CWD_LOCK can block on it, so holding a std
// guard across the await cannot deadlock the test runtime.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn report_default_output_uses_journal_stem() {
    // The default output lands in the cwd, which is process-global.
    let _guard = CWD_LOCK.lock().unwrap();
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
    let html = html_report(&journal, Some("https://tempo.example/"), "deadbeef");
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
    let html = html_report(&journal, None, "deadbeef");
    assert!(!html.contains("<a class=\"trace-link\""));
    assert!(html.contains(r#"<span class="trace-link">aabbccddeeff0011</span>"#));
}

#[test]
fn html_surfaces_failure_detail() {
    let journal = journal_with_failure();
    let html = html_report(&journal, None, "deadbeef");
    assert!(html.contains(r#"<span class="detail-error">connection refused on port 5432</span>"#));
    // No fabricated diagnostic command is emitted (the core path has no such field).
    assert!(!html.contains("tumult replay --trace"));
    // Failed rows carry the row-failed class.
    assert!(html.contains(r#"<tr class="row-failed">"#));
}

#[test]
fn html_footer_has_version_and_hash() {
    let journal = journal_with_failure();
    let html = html_report(&journal, None, "0123abcd0123abcd");
    assert!(html.contains(concat!("Tumult</strong> v", env!("CARGO_PKG_VERSION"))));
    assert!(html.contains("Report generated: 2026-11-17T00:00:00Z<br>"));
    assert!(html.contains("<code>0123abcd0123abcd</code>"));
}

#[test]
fn html_footer_names_the_resilience_intelligence_platform() {
    let html = html_report(&journal_with_failure(), None, "hash");
    assert!(html.contains("Rust-native resilience intelligence platform"));
    assert!(!html.contains("chaos engineering platform"));
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
        mttr_s: Some(5.0),
        impact_observed: None,
        outage_started_at_ns: None,
        recovered_at_ns: None,
    });
    write_journal(&journal, &journal_path).unwrap();

    // A recovery inside the target counts in full, so the verdict is the
    // recovery-aware one and not the pass-rate fallback.
    let (result, printed) = crate::output::testing::capture(|| {
        cmd_compliance(Some(&journal_path), ComplianceFramework::Soc2, false)
    });

    result.unwrap();
    let out = printed.stdout;
    assert!(
        out.contains(
            "  Pass rate: 100.0%\n  \
             Recovery within target: 100.0% of the runs that measured a recovery \
             (1 of 1; 0 against their own RTO, 1 against 30s)\n  \
             Evidence verdict: COMPLIANT\n"
        ),
        "{out}"
    );
}

#[test]
fn the_provenance_hash_of_an_unreadable_journal_is_an_error_not_a_hash() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("gone.toon");

    let err = crate::commands::report::source_hash(&missing).unwrap_err();
    assert!(format!("{err:#}").contains("gone.toon"), "{err:#}");
}

/// The digest of `abc` in FIPS 180-2, appendix B.1.
const SHA256_OF_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

#[test]
fn the_provenance_hash_is_the_sha256_of_the_bytes_of_the_journal() {
    let dir = TempDir::new().unwrap();
    let journal = dir.path().join("journal.toon");
    std::fs::write(&journal, b"abc").unwrap();

    let hash = crate::commands::report::source_hash(&journal).unwrap();

    assert_eq!(hash, SHA256_OF_ABC);
}

#[test]
fn the_report_footer_names_the_hash_as_sha256() {
    let html = html_report(&journal_with_failure(), None, SHA256_OF_ABC);

    assert!(
        html.contains(&format!(
            "Source journal SHA-256: <code>{SHA256_OF_ABC}</code>"
        )),
        "{html}"
    );
    assert!(!html.contains("non-cryptographic"), "{html}");
}
