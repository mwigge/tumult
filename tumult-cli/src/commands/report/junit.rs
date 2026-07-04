use std::fmt::Write as _;

use anyhow::Result;

use tumult_core::types::{ActivityResult, ActivityStatus, Journal};

use super::escape::xml_escape;

/// FIX 1: raw journal serialized as JSON.
///
/// # Errors
///
/// Returns an error if the journal cannot be serialized.
pub(crate) fn generate_json_report(journal: &Journal) -> Result<String> {
    Ok(serde_json::to_string_pretty(journal)?)
}

/// FIX 1: minimal `JUnit` XML — one `<testcase>` per activity across all phases.
pub(crate) fn generate_junit_report(journal: &Journal) -> String {
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
