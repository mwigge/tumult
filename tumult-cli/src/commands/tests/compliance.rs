//! Tests for `tumult compliance`: directory scanning (including malformed
//! journals), the missing-path and missing-journals errors, and the
//! reduced-assurance verdict when journals carry no recovery signals.

use super::super::*;
use super::helpers::*;
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

    cmd_compliance(Some(dir.path()), ComplianceFramework::Dora, false).unwrap();
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
fn compliance_without_recovery_signals_falls_back_to_pass_rate_only() {
    // No post_result and no analysis: recovery_compliance is None, so the
    // verdict must be the reduced-assurance pass-rate-only path.
    let dir = TempDir::new().unwrap();
    let mut journal = journal_with_failure();
    journal.status = ExperimentStatus::Completed;
    assert!(journal.post_result.is_none() && journal.analysis.is_none());
    let path = dir.path().join("plain.toon");
    tumult_core::journal::write_journal(&journal, &path).unwrap();

    cmd_compliance(Some(&path), ComplianceFramework::PciDss, false).unwrap();
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
        cmd_compliance(None, framework, true).unwrap();
    }
}
