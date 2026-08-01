//! Manual-evidence lifecycle tests (moved out of `src/manual.rs`): draft →
//! submit → verify/reject with segregation of duties, hash-chained audit,
//! attachments, bulk import, and the v7 → v8 index-free migration.

#![cfg(feature = "duckdb")]

use tumult_lake::{
    AttachmentKind, ExerciseType, ManualError, ManualOutcome, NewManualExperiment, Store, Writer,
    CURRENT_SCHEMA_VERSION,
};

const DAY: i64 = 86_400 * 1_000_000_000;

fn temp_writer() -> (tempfile::TempDir, Store, Writer) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    (d, store, writer)
}

fn draft(by: &str) -> NewManualExperiment {
    NewManualExperiment {
        experiment_name: "edge-cache-outage".into(),
        exercise_type: ExerciseType::GameDay,
        executed_at_ns: 100 * DAY,
        hypothesis: "Static asset failover keeps p95 under 800ms".into(),
        method: "Disabled the primary CDN PoP; observed failover".into(),
        outcome: ManualOutcome::Passed,
        hypothesis_met: Some(true),
        findings: Some("Failover worked; warm-up took 40s".into()),
        action_items: vec!["Pre-warm the secondary PoP".into()],
        target_system: Some("cdn".into()),
        target_environment: Some("production".into()),
        blast_radius: Some("single-pop".into()),
        recovery_time_s: Some(40.0),
        duration_s: Some(3600.0),
        entered_by: by.into(),
        attestation: "I attest this record reflects the exercise as executed.".into(),
        renewal_due_ns: Some(190 * DAY),
        framework_refs: vec!["DORA Art. 24(7)".into()],
    }
}

#[test]
fn create_submit_verify_happy_path() {
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&draft("alice")).unwrap();
    assert_eq!(id.len(), 26);

    writer.submit_manual(&id, None, "alice").unwrap();
    writer
        .verify_manual(&id, "bob", Some("evidence reviewed"))
        .unwrap();

    let reader = store.read_only().unwrap();
    let detail = reader.manual_experiment_detail(&id).unwrap().unwrap();
    assert_eq!(detail.experiment["status"], serde_json::json!("verified"));
    assert_eq!(detail.experiment["reviewed_by"], serde_json::json!("bob"));
    assert_eq!(
        detail.experiment["framework_refs"],
        serde_json::json!(["DORA Art. 24(7)"])
    );
    // Audit: create + submit + verify, hash chain intact.
    let actions: Vec<&str> = detail
        .audit
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["create", "submit", "verify"]);
    assert!(detail.audit[0]["prev_hash"].is_null());
    for w in detail.audit.windows(2) {
        assert_eq!(w[0]["new_hash"], w[1]["prev_hash"]);
    }
    let last = detail.audit.last().unwrap()["new_hash"].clone();
    assert_eq!(last, detail.experiment["content_hash"]);
}

#[test]
fn draft_edit_then_submit_locks() {
    let (_d, _store, writer) = temp_writer();
    let id = writer.create_manual_draft(&draft("alice")).unwrap();

    let mut edited = draft("alice");
    edited.findings = Some("updated findings".into());
    writer.update_manual_draft(&id, &edited, "alice").unwrap();

    writer.submit_manual(&id, None, "alice").unwrap();
    // Edits after submit are rejected — the record is locked.
    let err = writer
        .update_manual_draft(&id, &edited, "alice")
        .unwrap_err();
    assert!(matches!(err, ManualError::WrongStatus { .. }));
}

#[test]
fn self_review_is_rejected() {
    let (_d, _store, writer) = temp_writer();
    let id = writer.create_manual_draft(&draft("alice")).unwrap();
    writer.submit_manual(&id, None, "alice").unwrap();
    let err = writer.verify_manual(&id, "alice", None).unwrap_err();
    assert!(matches!(err, ManualError::SelfReview));
    let err = writer.reject_manual(&id, "alice", "no").unwrap_err();
    assert!(matches!(err, ManualError::SelfReview));
}

/// The v2–v7 `manual_experiments` shape (primary key + secondary
/// indexes) as a raw DDL fixture, so the v8 index-free rebuild can be
/// exercised end to end — the same crash-robustness amendment as the v5
/// run tables.
const V7_MANUAL_EXPERIMENTS_DDL: &str = "
CREATE TABLE schema_meta (key VARCHAR PRIMARY KEY, value BIGINT NOT NULL);
INSERT INTO schema_meta (key, value) VALUES ('version', 7);
CREATE TABLE manual_experiments (
    id VARCHAR PRIMARY KEY,
    experiment_name VARCHAR NOT NULL,
    exercise_type VARCHAR NOT NULL,
    executed_at_ns BIGINT NOT NULL,
    hypothesis VARCHAR NOT NULL,
    method VARCHAR NOT NULL,
    outcome_status VARCHAR NOT NULL,
    hypothesis_met BOOLEAN,
    findings VARCHAR,
    action_items JSON,
    target_system VARCHAR,
    target_environment VARCHAR,
    blast_radius VARCHAR,
    recovery_time_s DOUBLE,
    duration_s DOUBLE,
    origin VARCHAR NOT NULL DEFAULT 'manual',
    entered_by VARCHAR NOT NULL,
    entered_at_ns BIGINT NOT NULL,
    attestation VARCHAR NOT NULL,
    status VARCHAR NOT NULL DEFAULT 'draft',
    reviewed_by VARCHAR,
    reviewed_at_ns BIGINT,
    review_note VARCHAR,
    renewal_due_ns BIGINT,
    framework_refs VARCHAR[],
    batch_id VARCHAR,
    content_hash VARCHAR NOT NULL
);
CREATE INDEX idx_manual_experiments_name ON manual_experiments (experiment_name);
CREATE INDEX idx_manual_experiments_status ON manual_experiments (status);
";

#[test]
fn v7_store_migrates_to_index_free_manual_experiments() {
    let d = tempfile::TempDir::new().unwrap();
    let db = d.path().join("kronika.duckdb");
    // Build a v7-era store with a raw connection (Store::open would
    // migrate immediately).
    {
        let conn = duckdb::Connection::open(&db).unwrap();
        conn.execute_batch(V7_MANUAL_EXPERIMENTS_DDL).unwrap();
        conn.execute_batch(
            "INSERT INTO manual_experiments (id, experiment_name, exercise_type, \
             executed_at_ns, hypothesis, method, outcome_status, entered_by, \
             entered_at_ns, attestation, status, content_hash) \
             VALUES ('m-old', 'old gameday', 'gameday', 1, 'h', 'm', 'passed', \
             'alice', 1, 'att', 'submitted', 'hash-old');",
        )
        .unwrap();
    }

    let store = Store::open(&db).unwrap();
    let writer = store.writer().unwrap();
    assert_eq!(writer.schema_version().unwrap(), CURRENT_SCHEMA_VERSION);

    // Data survived the rebuild…
    let reader = store.read_only().unwrap();
    let detail = reader.manual_experiment_detail("m-old").unwrap().unwrap();
    assert_eq!(detail.experiment["status"], serde_json::json!("submitted"));
    assert_eq!(detail.experiment["origin"], serde_json::json!("manual"));

    // …and — the whole point — lifecycle UPDATEs work without any ART
    // index (the T5 desync bug class).
    writer
        .verify_manual("m-old", "bob", Some("reviewed"))
        .unwrap();
    let reader = store.read_only().unwrap();
    let detail = reader.manual_experiment_detail("m-old").unwrap().unwrap();
    assert_eq!(detail.experiment["status"], serde_json::json!("verified"));
    assert_eq!(detail.experiment["reviewed_by"], serde_json::json!("bob"));

    // No indexes remain on manual_experiments; the INSERT-only audit and
    // attachment tables keep theirs (desynced indexes can never break an
    // INSERT-only table's writes).
    let index_rows = reader
        .query_json_rows(
            "SELECT index_name, table_name FROM duckdb_indexes() \
             WHERE table_name = 'manual_experiments'",
        )
        .unwrap();
    assert!(index_rows.is_empty(), "{index_rows:?}");

    // Re-opening is a no-op (version already current, no rebuild attempted).
    drop(store);
    let store = Store::open(&db).unwrap();
    assert_eq!(
        store.writer().unwrap().schema_version().unwrap(),
        CURRENT_SCHEMA_VERSION
    );
    assert!(store
        .read_only()
        .unwrap()
        .manual_experiment_detail("m-old")
        .unwrap()
        .is_some());
}

#[test]
fn reject_requires_note_and_wrong_status_is_conflict() {
    let (_d, _store, writer) = temp_writer();
    let id = writer.create_manual_draft(&draft("alice")).unwrap();
    // Verify requires submitted first.
    let err = writer.verify_manual(&id, "bob", None).unwrap_err();
    assert!(matches!(err, ManualError::WrongStatus { .. }));
    writer.submit_manual(&id, None, "alice").unwrap();
    let err = writer.reject_manual(&id, "bob", "  ").unwrap_err();
    assert!(matches!(err, ManualError::Invalid(_)));
    writer
        .reject_manual(&id, "bob", "insufficient evidence")
        .unwrap();
}

#[test]
fn attachments_chain_audit_without_changing_hash() {
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&draft("alice")).unwrap();
    let attachment = writer
        .add_manual_attachment(
            &id,
            AttachmentKind::Url,
            "https://wiki.example.com/gameday-2026-07",
            Some("write-up"),
            None,
            "alice",
        )
        .unwrap();
    writer.submit_manual(&id, None, "alice").unwrap();
    writer.verify_manual(&id, "bob", None).unwrap();
    // Verified records are locked for attachments too.
    let err = writer
        .add_manual_attachment(&id, AttachmentKind::Url, "https://x", None, None, "bob")
        .unwrap_err();
    assert!(matches!(err, ManualError::WrongStatus { .. }));

    let reader = store.read_only().unwrap();
    let detail = reader.manual_experiment_detail(&id).unwrap().unwrap();
    assert_eq!(detail.attachments.len(), 1);
    assert_eq!(detail.attachments[0]["id"], serde_json::json!(attachment));
    // Attach audit row chains prev == new (content unchanged).
    let attach = detail
        .audit
        .iter()
        .find(|a| a["action"] == serde_json::json!("attach"))
        .unwrap();
    assert_eq!(attach["prev_hash"], attach["new_hash"]);
}

#[test]
fn bulk_import_lands_as_drafts_with_batch() {
    let (_d, store, writer) = temp_writer();
    let items = vec![draft("alice"), {
        let mut d = draft("carol");
        d.experiment_name = "db-failover".into();
        d.exercise_type = ExerciseType::Failover;
        d.outcome = ManualOutcome::Partial;
        d
    }];
    let (batch_id, ids) = writer
        .import_manual_drafts(&items, Some("q3-backfill".into()))
        .unwrap();
    assert_eq!(ids.len(), 2);

    let reader = store.read_only().unwrap();
    let rows = reader.manual_experiments(Some("draft")).unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows
        .iter()
        .all(|r| r["batch_id"] == serde_json::json!(batch_id)));
    let batches = reader
        .query_json_rows("SELECT source, rows, label FROM import_batches")
        .unwrap();
    assert_eq!(batches[0]["source"], serde_json::json!("manual-api"));
    assert_eq!(batches[0]["rows"], serde_json::json!(2));
}

#[test]
fn import_validates_every_item_and_rolls_back() {
    let (_d, store, writer) = temp_writer();
    let mut bad = draft("alice");
    bad.hypothesis = String::new();
    let err = writer
        .import_manual_drafts(&[draft("bob"), bad], None)
        .unwrap_err();
    assert!(matches!(err, ManualError::Invalid(_)));
    let reader = store.read_only().unwrap();
    assert_eq!(reader.manual_experiments(None).unwrap().len(), 0);
}

#[test]
fn missing_record_is_not_found() {
    let (_d, _store, writer) = temp_writer();
    let err = writer.submit_manual("01JNONE", None, "alice").unwrap_err();
    assert!(matches!(err, ManualError::NotFound(_)));
}
