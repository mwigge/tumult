//! Tests for `tumult compliance`: directory scanning (including malformed
//! journals), the missing-path and missing-journals errors, and the
//! reduced-assurance verdict when no run watched for recovery.

use super::super::*;
use super::helpers::*;
use crate::output::testing::capture;
use tempfile::TempDir;
use tumult_core::types::ExperimentStatus;

#[test]
fn compliance_scans_directory_and_skips_malformed_journals() {
    let dir = TempDir::new().unwrap();
    tumult_core::journal::write_journal(&journal_with_failure(), &dir.path().join("run.toon"))
        .unwrap();
    std::fs::write(dir.path().join("broken.toon"), "not a journal {{{").unwrap();
    // Non-toon files are ignored entirely.
    std::fs::write(dir.path().join("notes.txt"), "hello").unwrap();

    let (result, printed) =
        capture(|| cmd_compliance(Some(dir.path()), ComplianceFramework::Dora, false));

    result.unwrap();
    let out = printed.stdout;
    let heading = format!(
        "=== {} ===\n",
        ComplianceFramework::Dora.to_core().full_name()
    );
    assert!(out.starts_with(&heading), "{out}");
    // Only the one readable journal counts, and it deviated.
    assert!(out.contains("Journals analyzed: 1\n"), "{out}");
    assert!(out.contains("  deviated: 1 run(s)\n"), "{out}");
    assert!(out.contains("  Pass rate: 0.0%\n"), "{out}");
    assert!(out.ends_with("=== End Report ===\n"), "{out}");
}

#[test]
fn compliance_missing_path_errors() {
    let dir = TempDir::new().unwrap();
    let err = cmd_compliance(
        Some(&dir.path().join("nope")),
        ComplianceFramework::Soc2,
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("path does not exist"), "{err}");
}

#[test]
fn compliance_without_journals_or_sources_errors() {
    let err = cmd_compliance(None, ComplianceFramework::Nis2, false).unwrap_err();
    assert!(
        err.to_string().contains("a journals path is required"),
        "{err}"
    );
}

#[test]
fn a_run_that_only_completed_is_no_sign_of_recovery() {
    // The run completed with a full score and never watched its target
    // come back. That is its pass, counted once, in the pass rate.
    let dir = TempDir::new().unwrap();
    let mut journal = journal_with_failure();
    journal.status = ExperimentStatus::Completed;
    journal.analysis = Some(tumult_core::types::AnalysisResult {
        estimate_accuracy: None,
        estimate_recovery_delta_s: None,
        trend: None,
        resilience_score: Some(1.0),
    });
    assert!(journal.post_result.is_none());
    let path = dir.path().join("plain.toon");
    tumult_core::journal::write_journal(&journal, &path).unwrap();

    let (result, printed) =
        capture(|| cmd_compliance(Some(&path), ComplianceFramework::PciDss, false));

    result.unwrap();
    let out = printed.stdout;
    assert!(
        out.contains(
            "  Pass rate: 100.0%\n  \
             Recovery within 30s: not measured, no run watched for recovery. \
             The verdict rests on the pass rate only (reduced assurance).\n  \
             Evidence verdict: COMPLIANT (pass-rate only)\n"
        ),
        "{out}"
    );
}

#[test]
fn compliance_sources_lists_registry_for_each_framework() {
    // The registry path needs no journals and covers every framework mapping.
    for framework in [
        ComplianceFramework::Dora,
        ComplianceFramework::Nis2,
        ComplianceFramework::PciDss,
        ComplianceFramework::Iso22301,
        ComplianceFramework::Iso27001,
        ComplianceFramework::Soc2,
        ComplianceFramework::BaselIii,
    ] {
        let (result, printed) = capture(|| cmd_compliance(None, framework, true));

        result.unwrap();
        let out = printed.stdout;
        let core = framework.to_core();
        let heading = format!("=== {} — citation registry ===\n", core.full_name());
        assert!(out.starts_with(&heading), "{out}");
        assert!(
            out.contains(&format!(
                "Registry version: {}\n",
                tumult_core::compliance::REGISTRY_VERSION
            )),
            "{out}"
        );
        assert!(!core.citations().is_empty(), "{heading} has no citations");
        for citation in core.citations() {
            let line = format!("  {} — {}\n", citation.control_id, citation.title);
            assert!(out.contains(&line), "{line} missing from {out}");
        }
        // No report without journals.
        assert!(!out.contains("Journals analyzed"), "{out}");
    }
}
