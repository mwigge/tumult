//! Complete CLI backups respect `DuckDB`'s cross-process writer lock, and
//! carry the files attached to manual records both ways.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tumult_lake::{
    attachments_dir, ExerciseDetail, ExerciseType, FileUpload, ManualOutcome, NewManualExperiment,
    Store,
};

#[test]
fn backup_refuses_a_live_writer_and_succeeds_after_shutdown() {
    let dir = TempDir::new().unwrap();
    let db_path = dir.path().join("lake.duckdb");
    // A writer holding the exclusive lock stands in for the live daemon.
    let daemon = tumult_lake::AnalyticsStore::open(&db_path).unwrap();
    let out = dir.path().join("backup");
    let backup = || {
        Command::new(env!("CARGO_BIN_EXE_tumult"))
            .env("TUMULT_LAKE_PATH", &db_path)
            .args(["store", "backup", "--output"])
            .arg(&out)
            .output()
            .unwrap()
    };
    let locked = backup();
    assert!(
        !locked.status.success(),
        "a separate writer must prevent backup"
    );
    assert!(!out.exists(), "a refused backup must not publish a bundle");

    drop(daemon);
    let completed = backup();
    assert!(
        completed.status.success(),
        "{}",
        String::from_utf8_lossy(&completed.stderr)
    );

    assert!(out.join("store.duckdb").exists());
    assert!(out.join("manifest.json").exists());
}

/// A small PDF: the bytes start as every PDF does.
const PDF: &[u8] = b"%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\n%%EOF\n";

fn sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, b| {
            write!(hex, "{b:02x}").unwrap();
            hex
        })
}

/// `tumult store <args>` against the lake at `db`, with a private HOME.
fn tumult_store(home: &Path, db: &Path, args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tumult"))
        .env("HOME", home)
        .env("TUMULT_LAKE_PATH", db)
        .arg("store")
        .args(args)
        .output()
        .unwrap()
}

fn ok(out: &Output) -> String {
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A lake at `db` with a verified restore test that keeps the PDF as a
/// file; returns the record id.
fn lake_with_a_file(db: &Path) -> String {
    let store = Store::open(db).unwrap();
    let writer = store.writer().unwrap();
    let id = writer
        .create_manual_draft(&NewManualExperiment {
            experiment_name: "payments-db restore".into(),
            exercise_type: ExerciseType::Restore,
            executed_at_ns: 1_700_000_000_000_000_000,
            hypothesis: "The backup restores".into(),
            method: "Restore it on a spare host".into(),
            outcome: ManualOutcome::Passed,
            hypothesis_met: Some(true),
            findings: None,
            action_items: vec![],
            target_system: Some("payments-db".into()),
            target_environment: Some("production".into()),
            blast_radius: None,
            recovery_time_s: Some(600.0),
            duration_s: None,
            entered_by: "alice".into(),
            attestation: "I ran this restore".into(),
            renewal_due_ns: None,
            framework_refs: vec![],
            exercise: ExerciseDetail::default(),
        })
        .unwrap();
    let mut upload = FileUpload::begin(&attachments_dir(db)).unwrap();
    upload.write(PDF).unwrap();
    writer
        .add_manual_file(&id, upload.finish("runbook.pdf").unwrap(), "alice")
        .unwrap();
    writer.submit_manual(&id, None, "alice").unwrap();
    writer.verify_manual(&id, "bob", None).unwrap();
    id
}

fn stored(db: &Path, hash: &str) -> PathBuf {
    attachments_dir(db).join(&hash[..2]).join(hash)
}

#[test]
fn backup_and_restore_carry_the_files_attached_to_manual_records() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("live").join("lake.duckdb");
    let id = lake_with_a_file(&db);
    let hash = sha256(PDF);
    let bundle = dir.path().join("backup");

    let out = ok(&tumult_store(
        dir.path(),
        &db,
        &["backup".as_ref(), "--output".as_ref(), bundle.as_os_str()],
    ));
    assert!(out.contains("Attachment files: 1"), "{out}");
    let rel = format!("attachments/{}/{hash}", &hash[..2]);
    assert_eq!(std::fs::read(bundle.join(&rel)).unwrap(), PDF);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(bundle.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["attachments"][&hash], rel.as_str());
    assert_eq!(manifest["format_version"], 1);

    std::fs::create_dir(dir.path().join("restored")).unwrap();
    let restored = dir.path().join("restored").join("lake.duckdb");
    let out = ok(&tumult_store(
        dir.path(),
        &db,
        &[
            "restore".as_ref(),
            "--input".as_ref(),
            bundle.as_os_str(),
            "--output".as_ref(),
            restored.as_os_str(),
        ],
    ));
    assert!(out.contains("Attachment files restored: 1"), "{out}");
    assert_eq!(std::fs::read(stored(&restored, &hash)).unwrap(), PDF);
    let verdict = Store::at(&restored)
        .read_only()
        .unwrap()
        .verify_manual_record(&id)
        .unwrap()
        .unwrap();
    assert!(verdict.ok, "{verdict:?}");

    // A file that is there is never written over by a restore.
    std::fs::write(stored(&restored, &hash), b"kept as it was").unwrap();
    let again = dir.path().join("restored").join("again.duckdb");
    let out = ok(&tumult_store(
        dir.path(),
        &db,
        &[
            "restore".as_ref(),
            "--input".as_ref(),
            bundle.as_os_str(),
            "--output".as_ref(),
            again.as_os_str(),
        ],
    ));
    assert!(out.contains("Attachment files restored: 0"), "{out}");
    assert_eq!(
        std::fs::read(stored(&restored, &hash)).unwrap(),
        b"kept as it was"
    );
}

#[test]
fn a_backup_with_a_changed_file_is_not_restored() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("live").join("lake.duckdb");
    lake_with_a_file(&db);
    let hash = sha256(PDF);
    let bundle = dir.path().join("backup");
    ok(&tumult_store(
        dir.path(),
        &db,
        &["backup".as_ref(), "--output".as_ref(), bundle.as_os_str()],
    ));
    std::fs::write(
        bundle.join(format!("attachments/{}/{hash}", &hash[..2])),
        b"changed",
    )
    .unwrap();

    std::fs::create_dir(dir.path().join("restored")).unwrap();
    let restored = dir.path().join("restored").join("lake.duckdb");
    let out = tumult_store(
        dir.path(),
        &db,
        &[
            "restore".as_ref(),
            "--input".as_ref(),
            bundle.as_os_str(),
            "--output".as_ref(),
            restored.as_os_str(),
        ],
    );
    assert!(!out.status.success());
    assert!(!restored.exists(), "no database is restored");
    assert!(!stored(&restored, &hash).exists());
}

#[test]
fn a_backup_made_before_files_restores_with_no_files() {
    let dir = TempDir::new().unwrap();
    let db = dir.path().join("live").join("lake.duckdb");
    lake_with_a_file(&db);
    let bundle = dir.path().join("backup");
    ok(&tumult_store(
        dir.path(),
        &db,
        &["backup".as_ref(), "--output".as_ref(), bundle.as_os_str()],
    ));
    // As an older build wrote it: the manifest without the files, and no
    // attachments folder.
    let path = bundle.join("manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest.as_object_mut().unwrap().remove("attachments");
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    std::fs::remove_dir_all(bundle.join("attachments")).unwrap();

    std::fs::create_dir(dir.path().join("restored")).unwrap();
    let restored = dir.path().join("restored").join("lake.duckdb");
    let out = ok(&tumult_store(
        dir.path(),
        &db,
        &[
            "restore".as_ref(),
            "--input".as_ref(),
            bundle.as_os_str(),
            "--output".as_ref(),
            restored.as_os_str(),
        ],
    ));
    assert!(out.contains("Attachment files restored: 0"), "{out}");
    assert!(restored.is_file());
    assert!(!attachments_dir(&restored).exists());
}
