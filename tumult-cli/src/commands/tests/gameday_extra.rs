//! Tests for `tumult gameday create` happy paths (load/framework options) and
//! the `gameday run`/`gameday analyze` error boundaries not covered by the
//! inline wiring test.

use super::super::*;
use super::helpers::{CwdGuard, CWD_LOCK};
use tempfile::TempDir;

#[test]
fn gameday_create_renders_load_and_framework_options() {
    let _guard = CWD_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let _cwd = CwdGuard::enter(dir.path());

    cmd_gameday_create(
        "gd-full",
        &[std::path::PathBuf::from("exp.toon")],
        Some(LoadToolArg::K6),
        Some(std::path::Path::new("load.js")),
        Some(20),
        Some(ComplianceFramework::Dora),
    )
    .unwrap();

    let content = std::fs::read_to_string(dir.path().join("gd-full.gameday.toon")).unwrap();
    assert!(content.contains("title: gd-full"), "{content}");
    assert!(content.contains("tool: k6"), "{content}");
    assert!(content.contains("script: load.js"), "{content}");
    assert!(content.contains("vus: 20"), "{content}");
    assert!(content.contains("frameworks[1]: DORA"), "{content}");
    assert!(content.contains("- path: exp.toon"), "{content}");
}

#[test]
fn gameday_create_without_load_omits_load_block() {
    let _guard = CWD_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let _cwd = CwdGuard::enter(dir.path());

    // LoadToolArg::None explicitly disables load, same as passing no tool.
    cmd_gameday_create(
        "gd-plain",
        &[std::path::PathBuf::from("exp.toon")],
        Some(LoadToolArg::None),
        None,
        None,
        None,
    )
    .unwrap();

    let content = std::fs::read_to_string(dir.path().join("gd-plain.gameday.toon")).unwrap();
    assert!(!content.contains("load:"), "{content}");
    assert!(!content.contains("regulatory:"), "{content}");
}

#[test]
fn gameday_run_missing_file_errors() {
    let dir = TempDir::new().unwrap();
    let err = cmd_gameday_run(&dir.path().join("missing.gameday.toon")).unwrap_err();
    assert!(err.to_string().contains("failed to read"), "{err}");
}

#[test]
fn gameday_run_malformed_file_errors() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("broken.gameday.toon");
    std::fs::write(&path, "not a gameday {{{").unwrap();

    let err = cmd_gameday_run(&path).unwrap_err();
    assert!(err.to_string().contains("failed to parse gameday"), "{err}");
}

#[test]
fn gameday_run_reports_experiment_read_failure() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("gd.gameday.toon");
    std::fs::write(
        &path,
        "title: broken refs\n\nexperiments[1]:\n  - path: missing-exp.toon\n    compliance_maps[0]:\n\nscoring:\n  pass_threshold: 0.75\n  mttr_target_s: 30.0\n  recovery_required: true\n",
    )
    .unwrap();

    let err = cmd_gameday_run(&path).unwrap_err();
    assert!(
        err.to_string().contains("failed to read experiment"),
        "{err}"
    );
}

#[test]
fn gameday_analyze_missing_journal_errors() {
    let dir = TempDir::new().unwrap();
    let err = cmd_gameday_analyze(&dir.path().join("no-run.gameday.toon")).unwrap_err();
    assert!(err.to_string().contains("failed to read"), "{err}");
}

#[test]
fn gameday_analyze_malformed_journal_errors() {
    let dir = TempDir::new().unwrap();
    let gameday = dir.path().join("gd.gameday.toon");
    std::fs::write(&gameday, "title: x\n").unwrap();
    std::fs::write(gameday.with_extension("journal.toon"), "junk {{{").unwrap();

    let err = cmd_gameday_analyze(&gameday).unwrap_err();
    assert!(
        err.to_string().contains("failed to parse gameday journal"),
        "{err}"
    );
}
