//! Tests for `tumult export`: every output format, the derived output path,
//! and the missing-journal error.

use super::super::*;
use super::helpers::{journal_with_failure, CwdGuard, CWD_LOCK};
use tempfile::TempDir;

fn write_source_journal(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("run.toon");
    tumult_core::journal::write_journal(&journal_with_failure(), &path).unwrap();
    path
}

#[test]
fn export_json_round_trips_journal() {
    let _guard = CWD_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let journal = write_source_journal(dir.path());
    let _cwd = CwdGuard::enter(dir.path());

    cmd_export(&journal, ExportFormat::Json).unwrap();

    let json: tumult_core::types::Journal =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("run.json")).unwrap())
            .unwrap();
    assert_eq!(json, journal_with_failure());
}

#[test]
fn export_csv_writes_header_and_row() {
    let _guard = CWD_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let journal = write_source_journal(dir.path());
    let _cwd = CwdGuard::enter(dir.path());

    cmd_export(&journal, ExportFormat::Csv).unwrap();

    let csv = std::fs::read_to_string(dir.path().join("run.csv")).unwrap();
    assert!(csv.contains("experiment_id"), "{csv}");
    assert!(csv.contains("exp-fail-1"), "{csv}");
}

#[test]
fn export_parquet_and_arrow_write_nonempty_files() {
    let _guard = CWD_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let journal = write_source_journal(dir.path());
    let _cwd = CwdGuard::enter(dir.path());

    cmd_export(&journal, ExportFormat::Parquet).unwrap();
    cmd_export(&journal, ExportFormat::Arrow).unwrap();

    for ext in ["parquet", "arrow"] {
        let path = dir.path().join(format!("run.{ext}"));
        let len = std::fs::metadata(&path).unwrap().len();
        assert!(len > 0, "{ext} export must be non-empty");
    }
}

#[test]
fn export_missing_journal_errors() {
    let _guard = CWD_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let _cwd = CwdGuard::enter(dir.path());

    let err = cmd_export(&dir.path().join("missing.toon"), ExportFormat::Json).unwrap_err();
    assert!(err.to_string().contains("failed to read journal"), "{err}");
}
