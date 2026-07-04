//! Journal report renderers shared by the CLI and MCP server.
//!
//! JSON (the raw journal serialized) and `JUnit` XML (one `<testcase>` per
//! activity across all phases). Richer formats (HTML/PDF) stay in the CLI.

use std::fmt::Write as _;

use crate::types::{ActivityResult, ActivityStatus, Journal};

/// HTML-safe escaping of `&`, `<`, `>`, and `"`.
#[must_use]
pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// XML-safe escaping. Extends [`html_escape`] with the apostrophe entity.
#[must_use]
pub fn xml_escape(s: &str) -> String {
    html_escape(s).replace('\'', "&apos;")
}

/// The raw journal serialized as pretty JSON.
///
/// # Errors
///
/// Returns an error if the journal cannot be serialized.
pub fn json_report(journal: &Journal) -> Result<String, serde_json::Error> {
    serde_json::to_string_pretty(journal)
}

/// Minimal `JUnit` XML — one `<testcase>` per activity across all phases.
#[must_use]
pub fn junit_report(journal: &Journal) -> String {
    // Collect (phase, activity) across every phase that carries ActivityResults.
    let mut cases: Vec<(&str, &ActivityResult)> = Vec::new();
    if let Some(ref hyp) = journal.steady_state_before {
        for r in &hyp.probe_results {
            cases.push(("hypothesis_before", r));
        }
    }
    for r in &journal.method_results {
        cases.push(("method", r));
    }
    if let Some(ref hyp) = journal.steady_state_after {
        for r in &hyp.probe_results {
            cases.push(("hypothesis_after", r));
        }
    }
    for r in &journal.rollback_results {
        cases.push(("rollback", r));
    }

    let tests = cases.len();
    let failures = cases
        .iter()
        .filter(|(_, r)| matches!(r.status, ActivityStatus::Failed | ActivityStatus::Timeout))
        .count();
    #[allow(clippy::cast_precision_loss)]
    let suite_time = journal.duration_ms as f64 / 1000.0;

    let mut out = String::new();
    let _ = writeln!(out, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        out,
        r#"<testsuite name="{}" tests="{}" failures="{}" time="{:.3}">"#,
        xml_escape(&journal.experiment_title),
        tests,
        failures,
        suite_time
    );
    for (phase, r) in &cases {
        #[allow(clippy::cast_precision_loss)]
        let case_time = r.duration_ms as f64 / 1000.0;
        let _ = write!(
            out,
            r#"  <testcase name="{}" classname="{}" time="{:.3}">"#,
            xml_escape(&r.name),
            xml_escape(phase),
            case_time
        );
        match r.status {
            ActivityStatus::Failed | ActivityStatus::Timeout => {
                let msg = r.error.as_deref().or(r.output.as_deref()).unwrap_or("");
                let _ = write!(
                    out,
                    r#"<failure message="{:?}">{}</failure>"#,
                    r.status,
                    xml_escape(msg)
                );
            }
            ActivityStatus::Skipped => {
                let _ = write!(out, "<skipped/>");
            }
            ActivityStatus::Succeeded => {}
        }
        let _ = writeln!(out, "</testcase>");
    }
    let _ = writeln!(out, "</testsuite>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escape_covers_all_entities() {
        assert_eq!(
            xml_escape(r#"<a href="x">&'"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&apos;"
        );
    }

    #[test]
    fn junit_report_emits_suite_and_cases() {
        let journal = Journal {
            experiment_title: "Report & test".into(),
            experiment_id: "exp-1".into(),
            status: crate::types::ExperimentStatus::Completed,
            started_at_ns: 0,
            ended_at_ns: 0,
            duration_ms: 1500,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![
                ActivityResult {
                    name: "step-ok".into(),
                    activity_type: crate::types::ActivityType::Action,
                    status: ActivityStatus::Succeeded,
                    started_at_ns: 0,
                    duration_ms: 100,
                    output: None,
                    error: None,
                    trace_id: crate::types::TraceId::empty(),
                    span_id: crate::types::SpanId::empty(),
                },
                ActivityResult {
                    name: "step-bad".into(),
                    activity_type: crate::types::ActivityType::Action,
                    status: ActivityStatus::Failed,
                    started_at_ns: 0,
                    duration_ms: 50,
                    output: None,
                    error: Some("boom <&>".into()),
                    trace_id: crate::types::TraceId::empty(),
                    span_id: crate::types::SpanId::empty(),
                },
            ],
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
        };
        let xml = junit_report(&journal);
        assert!(xml.contains(r#"<testsuite name="Report &amp; test" tests="2" failures="1""#));
        assert!(xml.contains(r#"<testcase name="step-ok" classname="method""#));
        assert!(xml.contains("boom &lt;&amp;&gt;"));
        assert!(xml.ends_with("</testsuite>\n"));

        let json = json_report(&journal).unwrap();
        assert!(json.contains("\"experiment_title\": \"Report & test\""));
    }
}
