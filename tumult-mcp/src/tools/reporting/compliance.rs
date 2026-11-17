//! Compliance summary tool over journals for a target framework.

use std::fmt::Write as _;

use crate::error::ToolError;
use crate::tools::StructuredReport;

use tumult_core::compliance::{
    compliance_verdict, ComplianceFramework, ComplianceSignals, DEFAULT_MTTR_TARGET_S,
    EVIDENCE_DISCLAIMER,
};

use super::for_each_journal;

/// Compliance summary over journals for a target framework (mirrors
/// `tumult compliance`, sharing its scoring via `tumult_core::compliance`).
///
/// # Errors
///
/// Returns a [`ToolError`] if the framework is unknown (listing the valid
/// values) or the journals cannot be read.
pub fn compliance(journals_path: &str, framework: &str) -> Result<StructuredReport, ToolError> {
    let framework = ComplianceFramework::parse(framework).map_err(ToolError::InvalidInput)?;

    let mut signals = ComplianceSignals::default();
    let (loaded, skipped) = for_each_journal(journals_path, |journal| {
        signals.accumulate(&journal);
    })?;

    let pass_rate = signals.pass_rate();
    let recovery_compliance = signals.recovery_compliance(DEFAULT_MTTR_TARGET_S);
    let verdict = compliance_verdict(pass_rate, recovery_compliance);

    // Citations from the single sourced, dated registry in
    // `tumult_core::compliance` — the same source of truth the CLI renders.
    let citations: Vec<serde_json::Value> = framework
        .citations()
        .iter()
        .map(|c| {
            serde_json::json!({
                "control_id": c.control_id,
                "title": c.title,
                "requires": c.summary,
                "evidence_type": c.evidence_type.as_str(),
                "strength": c.strength.as_str(),
                "evidence_note": c.evidence_note,
                "source_url": c.source_url,
                "last_verified": c.last_verified,
            })
        })
        .collect();

    let mut structured = serde_json::Map::new();
    structured.insert(
        "framework".into(),
        serde_json::json!(framework.as_report_str()),
    );
    structured.insert("pass_rate".into(), serde_json::json!(pass_rate));
    structured.insert(
        "recovery_compliance".into(),
        serde_json::json!(recovery_compliance),
    );
    structured.insert("verdict".into(), serde_json::json!(verdict));
    structured.insert("journals_evaluated".into(), serde_json::json!(loaded));
    structured.insert("disclaimer".into(), serde_json::json!(EVIDENCE_DISCLAIMER));
    structured.insert(
        "source_url".into(),
        serde_json::json!(framework.source_url()),
    );
    structured.insert("citations".into(), serde_json::json!(citations));

    let mut text = String::new();
    writeln!(text, "=== {} ===", framework.full_name()).ok();
    writeln!(text).ok();
    writeln!(text, "{EVIDENCE_DISCLAIMER}").ok();
    writeln!(text).ok();
    writeln!(text, "Journals analyzed: {loaded}").ok();
    if skipped > 0 {
        writeln!(text, "Skipped (unreadable): {skipped}").ok();
    }
    writeln!(
        text,
        "With regulatory tagging: {}",
        signals.journals_with_regulatory
    )
    .ok();
    writeln!(text).ok();
    writeln!(text, "Evidence summary (NOT a compliance determination):").ok();
    writeln!(text, "  Pass rate: {:.1}%", pass_rate * 100.0).ok();
    writeln!(text, "  {}", signals.recovery_line(DEFAULT_MTTR_TARGET_S)).ok();
    writeln!(text, "  Evidence verdict: {verdict}").ok();

    writeln!(text).ok();
    writeln!(text, "Source: {}", framework.source_url()).ok();
    writeln!(
        text,
        "Mapped controls (evidence toward, not proof of, compliance):"
    )
    .ok();
    for c in framework.citations() {
        writeln!(text, "  {} — {}", c.control_id, c.title).ok();
        writeln!(
            text,
            "    Evidence [{} / {}]: {}",
            c.strength.as_str(),
            c.evidence_type.as_str(),
            c.evidence_note
        )
        .ok();
        writeln!(
            text,
            "    Source: {} (last verified {})",
            c.source_url, c.last_verified
        )
        .ok();
    }

    Ok(StructuredReport { text, structured })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::write_run_journal;
    use tempfile::TempDir;

    #[test]
    fn compliance_rejects_unknown_framework_listing_values() {
        let err = compliance(".", "hipaa").expect_err("unknown framework must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("hipaa"), "must name the bad value: {msg}");
        assert!(
            msg.contains("dora") && msg.contains("basel-iii"),
            "must list valid values: {msg}"
        );
    }

    #[test]
    fn compliance_missing_path_is_not_found() {
        let err = compliance("/nonexistent/journals", "dora").expect_err("missing path");
        assert!(err.to_string().contains("does not exist"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn compliance_over_single_completed_journal() {
        let dir = TempDir::new().unwrap();
        let journal_path = write_run_journal(dir.path());

        let result = compliance(journal_path.to_str().unwrap(), "dora").unwrap();
        assert_eq!(result.structured["framework"], "DORA");
        assert_eq!(result.structured["journals_evaluated"], 1);
        let pass_rate = result.structured["pass_rate"].as_f64().unwrap();
        assert!((pass_rate - 1.0).abs() < f64::EPSILON);
        // The run completed and watched nothing come back: its pass is in
        // the pass rate, and there is no recovery to speak of.
        assert_eq!(
            result.structured["recovery_compliance"],
            serde_json::Value::Null
        );
        assert_eq!(result.structured["verdict"], "COMPLIANT (pass-rate only)");
        assert!(
            result.text.contains(
                "  Pass rate: 100.0%\n  \
                 Recovery within 30s: not measured, no run watched for recovery. \
                 The verdict rests on the pass rate only (reduced assurance).\n  \
                 Evidence verdict: COMPLIANT (pass-rate only)\n"
            ),
            "{}",
            result.text
        );
        assert!(result.text.contains("Digital Operational Resilience Act"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn compliance_over_directory_skips_non_journals() {
        let dir = TempDir::new().unwrap();
        write_run_journal(dir.path());
        // The experiment definition also sits in the dir and is not a journal.
        let result = compliance(dir.path().to_str().unwrap(), "soc2").unwrap();
        assert_eq!(result.structured["journals_evaluated"], 1);
        assert!(result.text.contains("Skipped (unreadable): 1"));
    }
}
