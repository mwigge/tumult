//! Files as evidence of a manual record: a file is stored once in the
//! lake folder by its SHA-256, its row keeps the hash, the type, the size
//! and the name, it may be attached in any status without changing the
//! seal, the audit verify checks the stored files and the attach rows, and
//! the lake export carries the files to a fresh lake.

#![cfg(feature = "duckdb")]

use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};
use tumult_lake::lake::{committed_file, export, restore_attachments, LakeConfig};
use tumult_lake::{
    attachments_dir, ExerciseDetail, ExerciseType, FileError, FileUpload, ManualOutcome,
    NewManualExperiment, Reader, StagedFile, Store, Writer, MAX_FILE_BYTES,
};

const DAY: i64 = 86_400 * 1_000_000_000;

/// The table of the attachments.
const TABLE: &str = "evidence_attachments";

/// A small PDF: the bytes start as every PDF does.
const PDF: &[u8] = b"%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\n\
    trailer << /Root 1 0 R >>\n%%EOF\n";

fn temp_writer() -> (tempfile::TempDir, Store, Writer) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    (d, store, writer)
}

fn restore_drill(by: &str) -> NewManualExperiment {
    NewManualExperiment {
        experiment_name: "payments-db restore".into(),
        exercise_type: ExerciseType::Restore,
        executed_at_ns: 95 * DAY,
        hypothesis: "Last night's backup restores within 900 s".into(),
        method: "Restore the backup on a spare host and read the orders back".into(),
        outcome: ManualOutcome::Passed,
        hypothesis_met: Some(true),
        findings: None,
        action_items: vec![],
        target_system: Some("payments-db".into()),
        target_environment: Some("production".into()),
        blast_radius: None,
        recovery_time_s: Some(600.0),
        duration_s: None,
        entered_by: by.into(),
        attestation: "I attest this record reflects the exercise as executed.".into(),
        renewal_due_ns: None,
        framework_refs: vec![],
        exercise: ExerciseDetail::default(),
    }
}

/// `bytes` written to an upload in the attachments folder of `store`,
/// seven bytes at a time, and staged as `name`.
fn stage(store: &Store, name: &str, bytes: &[u8]) -> StagedFile {
    let mut upload = FileUpload::begin(&attachments_dir(store.path())).unwrap();
    for chunk in bytes.chunks(7) {
        upload.write(chunk).unwrap();
    }
    upload.finish(name).unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut hex, b| {
            write!(hex, "{b:02x}").unwrap();
            hex
        })
}

/// Every file under `dir`, found by walking it.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found.sort();
    found
}

fn stored(store: &Store, hash: &str) -> PathBuf {
    attachments_dir(store.path()).join(&hash[..2]).join(hash)
}

/// A verified restore test with the PDF attached by alice; returns its id.
fn verified_with_a_file(store: &Store, writer: &Writer) -> String {
    let id = writer.create_manual_draft(&restore_drill("alice")).unwrap();
    writer
        .add_manual_file(&id, stage(store, "runbook.pdf", PDF), "alice")
        .unwrap();
    writer.submit_manual(&id, None, "alice").unwrap();
    writer.verify_manual(&id, "bob", None).unwrap();
    id
}

#[test]
fn a_file_is_stored_once_by_its_hash_and_its_row_keeps_the_hash_type_size_and_name() {
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&restore_drill("alice")).unwrap();
    let first = writer
        .add_manual_file(&id, stage(&store, "runbook.pdf", PDF), "alice")
        .unwrap();
    let second = writer
        .add_manual_file(&id, stage(&store, "copy of runbook.pdf", PDF), "alice")
        .unwrap();
    assert_ne!(first, second);

    let hash = sha256(PDF);
    assert_eq!(
        files_under(&attachments_dir(store.path())),
        [stored(&store, &hash)],
        "one stored file, and no temporary file left"
    );
    assert_eq!(std::fs::read(stored(&store, &hash)).unwrap(), PDF);

    let reader = store.read_only().unwrap();
    let detail = reader.manual_experiment_detail(&id).unwrap().unwrap();
    let rows = &detail.attachments;
    assert_eq!(rows.len(), 2);
    for (row, (attachment, name)) in rows
        .iter()
        .zip([(&first, "runbook.pdf"), (&second, "copy of runbook.pdf")])
    {
        assert_eq!(row["id"], attachment.as_str());
        assert_eq!(row["kind"], "file");
        assert_eq!(row["file_hash"], hash.as_str());
        assert_eq!(row["media_type"], "application/pdf");
        assert_eq!(row["size_bytes"], PDF.len());
        assert_eq!(row["label"], name);
        assert_eq!(
            row["uri"],
            format!("/api/manual/experiments/{id}/attachments/{attachment}/file")
        );
    }
    let attach: Vec<&Value> = detail
        .audit
        .iter()
        .filter(|a| a["action"] == "attach")
        .collect();
    assert_eq!(attach.len(), 2);
    let diff = &attach[0]["diff"];
    assert_eq!(diff["attachment_id"], first.as_str());
    assert_eq!(diff["file_hash"], hash.as_str());
    assert!(reader.verify_manual_record(&id).unwrap().unwrap().ok);
}

#[test]
fn a_file_over_the_cap_is_refused_while_it_is_written_and_leaves_nothing() {
    let (_d, store, _writer) = temp_writer();
    let dir = attachments_dir(store.path());
    let mut upload = FileUpload::begin(&dir).unwrap();
    // 25 MB, written out: the cap the docs and the API promise.
    let cap = 25 * 1024 * 1024;
    assert_eq!(MAX_FILE_BYTES, 26_214_400);
    upload.write(&vec![b'a'; cap]).unwrap();
    let err = upload.write(b"a").unwrap_err();
    assert!(matches!(err, FileError::TooLarge), "{err:?}");
    assert_eq!(
        err.to_string(),
        "the file is larger than 25 MB (26214400 bytes)"
    );
    drop(upload);
    assert_eq!(files_under(&dir), Vec::<PathBuf>::new());
}

#[test]
fn a_file_attached_after_verify_keeps_the_seal_and_adds_one_chained_audit_row() {
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&restore_drill("alice")).unwrap();
    writer.submit_manual(&id, None, "alice").unwrap();
    writer.verify_manual(&id, "bob", None).unwrap();
    let before = store
        .read_only()
        .unwrap()
        .manual_experiment_detail(&id)
        .unwrap()
        .unwrap();

    let attachment = writer
        .add_manual_file(
            &id,
            stage(&store, "restore.log", b"restored 1204 orders\n"),
            "carol",
        )
        .unwrap();

    let reader = store.read_only().unwrap();
    let after = reader.manual_experiment_detail(&id).unwrap().unwrap();
    assert_eq!(
        after.experiment, before.experiment,
        "the record is unchanged"
    );
    assert_eq!(after.audit.len(), before.audit.len() + 1);
    assert_eq!(after.audit[..before.audit.len()], before.audit[..]);
    let row = after.audit.last().unwrap();
    let seal = &before.experiment["content_hash"];
    assert_eq!(row["action"], "attach");
    assert_eq!(row["changed_by"], "carol");
    assert_eq!((&row["prev_hash"], &row["new_hash"]), (seal, seal));
    assert_eq!(row["link_hash"].as_str().map(str::len), Some(64));
    assert_eq!(after.attachments[0]["id"], attachment.as_str());
    assert_eq!(after.attachments[0]["media_type"], "text/plain");
    let verdict = reader.verify_manual_record(&id).unwrap().unwrap();
    assert!(verdict.ok, "{verdict:?}");
}

/// Whether `reader` finds a problem with row `row` of `table` in record
/// `id` that says `says`, with `field` when one is given.
fn names(
    reader: &Reader,
    id: &str,
    (table, row, field): (&str, &str, Option<&str>),
    says: &str,
) -> bool {
    let verdict = reader.verify_manual_record(id).unwrap().unwrap();
    !verdict.ok
        && verdict.problems.iter().any(|p| {
            p.table == table
                && p.row == row
                && field.is_none_or(|f| p.field.as_deref() == Some(f))
                && p.problem.contains(says)
        })
}

#[test]
fn verify_names_a_changed_file_a_deleted_file_a_moved_hash_and_a_deleted_attach_row() {
    let (_d, store, writer) = temp_writer();
    let id = verified_with_a_file(&store, &writer);
    let log = b"restored 1204 orders\n";
    let second = writer
        .add_manual_file(&id, stage(&store, "restore.log", log), "carol")
        .unwrap();
    let first = {
        let reader = store.read_only().unwrap();
        let detail = reader.manual_experiment_detail(&id).unwrap().unwrap();
        detail.attachments[0]["id"].as_str().unwrap().to_string()
    };
    let now = || store.read_only().unwrap();
    assert!(now().verify_manual_record(&id).unwrap().unwrap().ok);

    // A stored file changed on disk.
    let pdf = stored(&store, &sha256(PDF));
    std::fs::write(&pdf, b"%PDF-1.4 a different runbook").unwrap();
    let row = (TABLE, first.as_str(), Some("file_hash"));
    assert!(names(&now(), &id, row, "it was changed"));
    std::fs::write(&pdf, PDF).unwrap();
    assert!(now().verify_manual_record(&id).unwrap().unwrap().ok);

    // A stored file deleted.
    let path = stored(&store, &sha256(log));
    std::fs::remove_file(&path).unwrap();
    let row = (TABLE, second.as_str(), Some("file_hash"));
    assert!(names(&now(), &id, row, "the stored file is missing"));
    std::fs::write(&path, log).unwrap();
    assert!(now().verify_manual_record(&id).unwrap().unwrap().ok);

    // A row that names another stored file than its attach row did.
    let swap = |hash: &str| {
        format!("UPDATE evidence_attachments SET file_hash = '{hash}' WHERE id = '{second}'")
    };
    writer.execute(&swap(&sha256(PDF)), []).unwrap();
    let row = (TABLE, second.as_str(), Some("file_hash"));
    assert!(names(&now(), &id, row, "not what the attach row"));
    writer.execute(&swap(&sha256(log)), []).unwrap();
    assert!(now().verify_manual_record(&id).unwrap().unwrap().ok);

    // The newest attach row deleted: the seal chain cannot show it, since
    // an attach leaves the seal as it was.
    writer
        .execute(
            "DELETE FROM manual_experiment_audit WHERE action = 'attach' AND changed_by = 'carol'",
            [],
        )
        .unwrap();
    let row = (TABLE, second.as_str(), None);
    assert!(names(
        &now(),
        &id,
        row,
        "no audit row names this attachment"
    ));
}

#[test]
fn an_attach_row_that_names_a_deleted_attachment_is_a_problem() {
    let (_d, store, writer) = temp_writer();
    let id = verified_with_a_file(&store, &writer);
    let reader = store.read_only().unwrap();
    let detail = reader.manual_experiment_detail(&id).unwrap().unwrap();
    let attach = detail
        .audit
        .iter()
        .find(|a| a["action"] == "attach")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    drop(reader);
    writer
        .execute("DELETE FROM evidence_attachments", [])
        .unwrap();
    let reader = store.read_only().unwrap();
    let row = ("manual_experiment_audit", attach.as_str(), Some("diff"));
    assert!(names(
        &reader,
        &id,
        row,
        "which is not in evidence_attachments"
    ));
}

#[test]
fn a_file_for_a_record_that_is_not_there_is_not_kept() {
    let (_d, store, writer) = temp_writer();
    let err = writer
        .add_manual_file("no-such-record", stage(&store, "runbook.pdf", PDF), "alice")
        .unwrap_err();
    assert!(
        matches!(&err, tumult_lake::ManualError::NotFound(id) if id == "no-such-record"),
        "{err:?}"
    );
    assert_eq!(
        files_under(&attachments_dir(store.path())),
        Vec::<PathBuf>::new()
    );
}

/// The tables a manual record lives in.
const MANUAL_TABLES: [&str; 7] = [
    "manual_experiments",
    "manual_experiment_audit",
    "evidence_attachments",
    "manual_exercise_steps",
    "manual_action_items",
    "manual_experiment_links",
    "manual_signoffs",
];

/// A fresh lake filled from the archive of `cfg`: the manual tables as
/// their committed snapshots, and the files the archive carries.
fn import(cfg: &LakeConfig) -> (tempfile::TempDir, Store, usize) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open(&dir.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    for table in MANUAL_TABLES {
        let file = committed_file(cfg, table).unwrap();
        writer
            .execute(
                &format!(
                    "INSERT INTO {table} BY NAME SELECT * FROM read_parquet('{}')",
                    file.display()
                ),
                [],
            )
            .unwrap();
    }
    let restored = restore_attachments(cfg, &store).unwrap();
    (dir, store, restored)
}

#[test]
fn the_lake_export_carries_the_files_and_a_fresh_lake_gets_them_back() {
    let (dir, store, writer) = temp_writer();
    let id = verified_with_a_file(&store, &writer);
    let hash = sha256(PDF);
    let reader = store.read_only().unwrap();
    let cfg = LakeConfig::new(dir.path().join("lake"), 0);

    let report = export(&reader, &cfg).unwrap();
    assert_eq!(report.attachments, 1);
    assert!(report.missing_attachments.is_empty());
    let carried = cfg.dir.join("attachments").join(&hash[..2]).join(&hash);
    assert_eq!(std::fs::read(&carried).unwrap(), PDF);
    let meta: Value =
        serde_json::from_slice(&std::fs::read(cfg.dir.join("_meta.json")).unwrap()).unwrap();
    assert_eq!(
        meta["attachments"][&hash],
        format!("attachments/{}/{hash}", &hash[..2])
    );
    // A second export finds the file in place, and a copy that no longer
    // has the bytes of its hash is written again.
    assert_eq!(export(&reader, &cfg).unwrap().attachments, 1);
    std::fs::write(&carried, b"%PDF-1.4 damaged in the archive").unwrap();
    assert_eq!(export(&reader, &cfg).unwrap().attachments, 1);
    assert_eq!(std::fs::read(&carried).unwrap(), PDF);

    let (_other_dir, other, restored) = import(&cfg);
    assert_eq!(restored, 1);
    assert_eq!(std::fs::read(stored(&other, &hash)).unwrap(), PDF);
    let other_reader = other.read_only().unwrap();
    let row = &other_reader
        .manual_experiment_detail(&id)
        .unwrap()
        .unwrap()
        .attachments[0];
    assert_eq!(row["file_hash"], hash.as_str());
    let verdict = other_reader.verify_manual_record(&id).unwrap().unwrap();
    assert!(verdict.ok, "{verdict:?}");
    // A file in place is never written again.
    assert_eq!(restore_attachments(&cfg, &other).unwrap(), 0);

    // A file of the archive that no longer has the bytes of its hash is
    // not brought back.
    std::fs::write(&carried, b"%PDF-1.4 not the runbook").unwrap();
    let third_dir = tempfile::TempDir::new().unwrap();
    let third = Store::open(&third_dir.path().join("kronika.duckdb")).unwrap();
    assert!(restore_attachments(&cfg, &third).is_err());
    assert!(!stored(&third, &hash).exists());
}

#[test]
fn an_export_names_a_file_it_cannot_carry_and_still_exports_the_tables() {
    let (dir, store, writer) = temp_writer();
    verified_with_a_file(&store, &writer);
    let hash = sha256(PDF);
    std::fs::remove_file(stored(&store, &hash)).unwrap();
    let reader = store.read_only().unwrap();
    let cfg = LakeConfig::new(dir.path().join("lake"), 0);

    let report = export(&reader, &cfg).unwrap();
    assert_eq!(report.attachments, 0);
    assert_eq!(report.missing_attachments, [hash]);
    assert!(committed_file(&cfg, "evidence_attachments").is_ok());
}

#[test]
fn a_file_is_never_attached_as_a_link() {
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&restore_drill("alice")).unwrap();
    let err = writer
        .add_manual_attachment(
            &id,
            tumult_lake::AttachmentKind::File,
            "/etc/passwd",
            Some("runbook.pdf"),
            Some(&"ab".repeat(32)),
            "alice",
        )
        .unwrap_err();
    assert!(
        matches!(&err, tumult_lake::ManualError::Invalid(msg) if msg.contains("not as a link")),
        "{err:?}"
    );
    let reader = store.read_only().unwrap();
    let detail = reader.manual_experiment_detail(&id).unwrap().unwrap();
    assert!(detail.attachments.is_empty(), "{:?}", detail.attachments);
    assert_eq!(detail.audit.len(), 1, "only the create row");
}

/// Set the time `path` was last written to `age` ago.
fn written_ago(path: &Path, age: std::time::Duration) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(std::time::SystemTime::now() - age)
        .unwrap();
}

#[test]
fn stale_upload_files_are_removed_when_the_store_opens_and_logged() {
    let d = tempfile::TempDir::new().unwrap();
    let db = d.path().join("kronika.duckdb");
    let store = Store::open(&db).unwrap();
    let writer = store.writer().unwrap();
    let id = writer.create_manual_draft(&restore_drill("alice")).unwrap();
    writer
        .add_manual_file(&id, stage(&store, "runbook.pdf", PDF), "alice")
        .unwrap();
    drop(writer);
    drop(store);
    let dir = attachments_dir(&db);
    let hour = std::time::Duration::from_secs(3600);
    for (name, age) in [
        (".upload-cut-off-yesterday", 24 * hour),
        (".upload-cut-off-two-hours-ago", 2 * hour),
        (".upload-still-arriving", std::time::Duration::from_secs(60)),
    ] {
        std::fs::write(dir.join(name), b"partial").unwrap();
        written_ago(&dir.join(name), age);
    }
    // An old stored file is evidence, never swept.
    written_ago(&stored_path(&db, &sha256(PDF)), 48 * hour);

    let logs = tumult_otel::logging::testing::capture_json(tracing::Level::INFO, || {
        Store::open(&db).unwrap();
    });

    assert_eq!(
        files_under(&dir),
        [
            dir.join(".upload-still-arriving"),
            stored_path(&db, &sha256(PDF))
        ]
    );
    let swept = logs.named("lake.attachments.swept");
    assert_eq!(swept.len(), 1, "{}", logs.raw);
    assert_eq!(swept[0]["removed"], 2, "{}", logs.raw);
    // Nothing more to remove: nothing is logged.
    let logs = tumult_otel::logging::testing::capture_json(tracing::Level::INFO, || {
        Store::open(&db).unwrap();
    });
    assert!(
        logs.named("lake.attachments.swept").is_empty(),
        "{}",
        logs.raw
    );
}

fn stored_path(db: &Path, hash: &str) -> PathBuf {
    attachments_dir(db).join(&hash[..2]).join(hash)
}

/// The answer to `bytes` sent as `name`: the media type, or why not.
fn type_of(store: &Store, name: &str, bytes: &[u8]) -> Result<&'static str, String> {
    let mut upload = FileUpload::begin(&attachments_dir(store.path())).unwrap();
    upload.write(bytes).unwrap();
    upload
        .finish(name)
        .map(|staged| staged.media_type())
        .map_err(|e| e.to_string())
}

#[test]
fn a_jpeg_name_must_have_jpeg_bytes_and_a_name_may_be_255_characters() {
    let (_d, store, _writer) = temp_writer();
    for name in ["photo.jpg", "photo.JPEG"] {
        let err = type_of(&store, name, PDF).unwrap_err();
        assert!(
            err.contains("the name says image/jpeg, but the bytes are application/pdf"),
            "{name}: {err}"
        );
    }
    let longest = format!("{}.pdf", "a".repeat(251));
    assert_eq!(longest.chars().count(), 255);
    assert_eq!(type_of(&store, &longest, PDF), Ok("application/pdf"));
    let longer = format!("{}.pdf", "a".repeat(252));
    let err = type_of(&store, &longer, PDF).unwrap_err();
    assert_eq!(err, "the file name is longer than 255 characters");
}

#[test]
fn a_temporary_file_that_is_already_gone_is_not_logged_and_one_that_stays_is() {
    let (_d, store, _writer) = temp_writer();
    let dir = attachments_dir(store.path());
    let temp_of = || {
        let found: Vec<PathBuf> = files_under(&dir)
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(".upload-")
            })
            .collect();
        assert_eq!(found.len(), 1, "{found:?}");
        found[0].clone()
    };
    let level = tracing::Level::WARN;

    // Removed by someone else first: nothing to say.
    let upload = FileUpload::begin(&dir).unwrap();
    std::fs::remove_file(temp_of()).unwrap();
    let logs = tumult_otel::logging::testing::capture_json(level, || drop(upload));
    assert!(logs.events.is_empty(), "{}", logs.raw);

    // In its place a folder, which a file removal cannot take: a warning.
    let upload = FileUpload::begin(&dir).unwrap();
    let temp = temp_of();
    std::fs::remove_file(&temp).unwrap();
    std::fs::create_dir(&temp).unwrap();
    let logs = tumult_otel::logging::testing::capture_json(level, || drop(upload));
    assert_eq!(logs.events.len(), 1, "{}", logs.raw);
    assert!(
        logs.raw.contains("could not remove a temporary file"),
        "{}",
        logs.raw
    );
}

#[cfg(unix)]
#[test]
fn a_file_whose_place_cannot_be_looked_at_is_not_stored() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&restore_drill("alice")).unwrap();
    let hash = sha256(PDF);
    let folder = attachments_dir(store.path()).join(&hash[..2]);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o000)).unwrap();
    // Where the folder can still be read (as root), the case cannot be made.
    let shut = std::fs::read_dir(&folder).is_err();
    let staged = stage(&store, "runbook.pdf", PDF);
    let placed = writer.add_manual_file(&id, staged, "alice");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o700)).unwrap();
    if !shut {
        return;
    }
    let err = placed.unwrap_err();
    assert!(matches!(err, tumult_lake::ManualError::Store(_)), "{err:?}");
    assert!(
        !folder.join(&hash).exists(),
        "never written past a check that failed"
    );
    let reader = store.read_only().unwrap();
    let detail = reader.manual_experiment_detail(&id).unwrap().unwrap();
    assert!(detail.attachments.is_empty());
}

#[test]
fn the_sweep_says_nothing_without_a_folder_and_warns_when_it_cannot_read_one() {
    let level = tracing::Level::WARN;
    let d = tempfile::TempDir::new().unwrap();
    let db = d.path().join("kronika.duckdb");
    let logs = tumult_otel::logging::testing::capture_json(level, || {
        Store::open(&db).unwrap();
    });
    assert!(!attachments_dir(&db).exists());
    assert!(
        logs.named("lake.attachments.sweep_failed").is_empty(),
        "{}",
        logs.raw
    );

    // A file where the folder should be: the sweep cannot read it, says
    // so, and the store opens.
    std::fs::write(attachments_dir(&db), b"not a folder").unwrap();
    let logs = tumult_otel::logging::testing::capture_json(level, || {
        Store::open(&db).unwrap();
    });
    assert_eq!(
        logs.named("lake.attachments.sweep_failed").len(),
        1,
        "{}",
        logs.raw
    );
}
