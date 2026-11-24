//! Schema v22 manual DR records: planned and actual RTO and RPO, the data
//! verified flag, the `restore` and `backup` types, participants, steps,
//! actions, links, and the upgrade of a v21 lake.

#![cfg(feature = "duckdb")]

use serde_json::{json, Value};
use tumult_lake::{
    ActionItem, ExerciseDetail, ExerciseLink, ExerciseStep, ExerciseType, LinkKind, ManualError,
    ManualOutcome, NewManualExperiment, Participant, StepResult, Store, Writer,
    CURRENT_SCHEMA_VERSION,
};

const DAY: i64 = 86_400 * 1_000_000_000;
const MINUTE: i64 = 60 * 1_000_000_000;

fn temp_writer() -> (tempfile::TempDir, Store, Writer) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    (d, store, writer)
}

fn dr_detail() -> ExerciseDetail {
    ExerciseDetail {
        rto_target_s: Some(3600.0),
        rpo_target_s: Some(300.0),
        rpo_measured_s: Some(120.0),
        data_verified: Some(true),
        participants: vec![
            Participant {
                name: "alice".into(),
                role: Some("dr lead".into()),
            },
            Participant {
                name: "carol".into(),
                role: None,
            },
        ],
        steps: vec![
            ExerciseStep {
                title: "Stop the primary".into(),
                actor: Some("alice".into()),
                started_at_ns: Some(100 * DAY),
                ended_at_ns: Some(100 * DAY + MINUTE),
                result: Some(StepResult::Done),
                note: None,
            },
            ExerciseStep {
                title: "Restore last night's backup".into(),
                actor: Some("carol".into()),
                started_at_ns: Some(100 * DAY + MINUTE),
                ended_at_ns: Some(100 * DAY + 50 * MINUTE),
                result: Some(StepResult::Done),
                note: Some("restored to the 02:14 snapshot".into()),
            },
        ],
        actions: vec![ActionItem {
            title: "Tune restore parallelism".into(),
            owner: "carol".into(),
            due_at_ns: Some(130 * DAY),
            ticket: Some("OPS-12".into()),
            severity: None,
            retest: None,
        }],
        links: vec![
            ExerciseLink {
                kind: LinkKind::Function,
                linked_id: "cif-0001".into(),
                label: Some("payments".into()),
            },
            ExerciseLink {
                kind: LinkKind::Run,
                linked_id: "exp-restore-1".into(),
                label: None,
            },
        ],
        plan_id: None,
        findings_severity: None,
        tester: None,
        tlpt: None,
        scenarios: Vec::new(),
    }
}

fn restore_test(by: &str) -> NewManualExperiment {
    NewManualExperiment {
        experiment_name: "payments-db-restore".into(),
        exercise_type: ExerciseType::Restore,
        executed_at_ns: 100 * DAY,
        hypothesis: "The payments database restores within 3600 s".into(),
        method: "Restore last night's backup to the standby".into(),
        outcome: ManualOutcome::Passed,
        hypothesis_met: Some(true),
        findings: None,
        action_items: vec![],
        target_system: Some("payments-db".into()),
        target_environment: Some("production".into()),
        blast_radius: None,
        recovery_time_s: Some(3000.0),
        duration_s: Some(3600.0),
        entered_by: by.into(),
        attestation: "I attest this record reflects the exercise as executed.".into(),
        renewal_due_ns: None,
        framework_refs: vec!["DORA Art. 12(2)".into()],
        exercise: dr_detail(),
    }
}

fn titles(rows: &[Value]) -> Vec<&str> {
    rows.iter().map(|r| r["title"].as_str().unwrap()).collect()
}

#[test]
fn a_restore_test_keeps_its_targets_steps_actions_and_links() {
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&restore_test("alice")).unwrap();

    let detail = store
        .read_only()
        .unwrap()
        .manual_experiment_detail(&id)
        .unwrap()
        .unwrap();
    let row = &detail.experiment;
    assert_eq!(row["exercise_type"], "restore");
    assert_eq!(row["recovery_time_s"], 3000.0);
    assert_eq!(row["rto_target_s"], 3600.0);
    assert_eq!(row["rpo_target_s"], 300.0);
    assert_eq!(row["rpo_measured_s"], 120.0);
    assert_eq!(row["data_verified"], true);
    assert_eq!(row["plan_id"], Value::Null);
    assert_eq!(row["hash_version"], 2);
    assert_eq!(
        row["participants"],
        json!([
            {"name": "alice", "role": "dr lead"},
            {"name": "carol", "role": null}
        ])
    );

    assert_eq!(
        titles(&detail.steps),
        ["Stop the primary", "Restore last night's backup"]
    );
    let ordinals: Vec<i64> = detail
        .steps
        .iter()
        .map(|s| s["ordinal"].as_i64().unwrap())
        .collect();
    assert_eq!(ordinals, [1, 2]);
    assert_eq!(detail.steps[1]["actor"], "carol");
    assert_eq!(detail.steps[1]["result"], "done");
    assert_eq!(detail.steps[1]["note"], "restored to the 02:14 snapshot");
    assert_eq!(detail.steps[1]["ended_at_ns"], 100 * DAY + 50 * MINUTE);

    assert_eq!(detail.actions.len(), 1);
    let action = &detail.actions[0];
    assert_eq!(action["title"], "Tune restore parallelism");
    assert_eq!(action["owner"], "carol");
    assert_eq!(action["due_at_ns"], 130 * DAY);
    assert_eq!(action["ticket"], "OPS-12");
    assert_eq!(action["status"], "open");
    assert_eq!(action["closed_at_ns"], Value::Null);

    let links: Vec<(&str, &str)> = detail
        .links
        .iter()
        .map(|l| {
            (
                l["kind"].as_str().unwrap(),
                l["linked_id"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(links, [("function", "cif-0001"), ("run", "exp-restore-1")]);

    // The create audit row seals the record as stored.
    assert_eq!(detail.audit[0]["new_hash"], row["content_hash"]);
}

#[test]
fn a_record_with_no_dr_detail_reads_with_empty_lists() {
    let (_d, store, writer) = temp_writer();
    let new = NewManualExperiment {
        exercise: ExerciseDetail::default(),
        ..restore_test("alice")
    };
    let id = writer.create_manual_draft(&new).unwrap();
    let detail = store
        .read_only()
        .unwrap()
        .manual_experiment_detail(&id)
        .unwrap()
        .unwrap();
    assert_eq!(detail.experiment["rto_target_s"], Value::Null);
    assert_eq!(detail.experiment["data_verified"], Value::Null);
    assert_eq!(detail.experiment["participants"], json!([]));
    assert!(detail.steps.is_empty());
    assert!(detail.actions.is_empty());
    assert!(detail.links.is_empty());
}

#[test]
fn a_draft_edit_replaces_the_steps_actions_and_links() {
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&restore_test("alice")).unwrap();

    let mut edited = restore_test("alice");
    edited.exercise.steps.truncate(1);
    edited.exercise.steps[0].title = "Fence the primary".into();
    edited.exercise.actions.clear();
    edited.exercise.links = vec![ExerciseLink {
        kind: LinkKind::GameDay,
        linked_id: "run-gd-7".into(),
        label: None,
    }];
    edited.exercise.rpo_measured_s = Some(400.0);
    edited.exercise.data_verified = Some(false);
    writer.update_manual_draft(&id, &edited, "alice").unwrap();

    let detail = store
        .read_only()
        .unwrap()
        .manual_experiment_detail(&id)
        .unwrap()
        .unwrap();
    assert_eq!(titles(&detail.steps), ["Fence the primary"]);
    assert!(detail.actions.is_empty());
    assert_eq!(detail.links.len(), 1);
    assert_eq!(detail.links[0]["kind"], "gameday");
    assert_eq!(detail.experiment["rpo_measured_s"], 400.0);
    assert_eq!(detail.experiment["data_verified"], false);

    let edit = detail.audit.iter().find(|a| a["action"] == "edit").unwrap();
    let fields: Vec<&str> = edit["diff"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    for field in [
        "rto_target_s",
        "rpo_target_s",
        "rpo_measured_s",
        "data_verified",
        "participants",
        "steps",
        "actions",
        "links",
    ] {
        assert!(fields.contains(&field), "{field} missing from {fields:?}");
    }
    assert_ne!(edit["prev_hash"], edit["new_hash"]);
    assert_eq!(edit["new_hash"], detail.experiment["content_hash"]);
}

#[test]
fn every_lifecycle_write_keeps_the_seal_chain_of_a_dr_record() {
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&restore_test("alice")).unwrap();
    writer.submit_manual(&id, None, "alice").unwrap();
    writer
        .verify_manual(&id, "bob", Some("restore log checked"))
        .unwrap();

    let detail = store
        .read_only()
        .unwrap()
        .manual_experiment_detail(&id)
        .unwrap()
        .unwrap();
    let actions: Vec<&str> = detail
        .audit
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["create", "submit", "verify"]);
    for w in detail.audit.windows(2) {
        assert_eq!(w[0]["new_hash"], w[1]["prev_hash"]);
        assert_ne!(w[1]["prev_hash"], w[1]["new_hash"]);
    }
    assert_eq!(
        detail.audit.last().unwrap()["new_hash"],
        detail.experiment["content_hash"]
    );
    assert_eq!(detail.experiment["hash_version"], 2);
    // The steps and actions stay as entered after sign-off.
    assert_eq!(detail.steps.len(), 2);
    assert_eq!(detail.actions.len(), 1);
}

#[test]
fn an_import_writes_the_dr_detail_of_every_record() {
    let (_d, store, writer) = temp_writer();
    let (_, ids) = writer
        .import_manual_drafts(&[restore_test("alice"), restore_test("carol")], None)
        .unwrap();
    let reader = store.read_only().unwrap();
    for id in ids {
        let detail = reader.manual_experiment_detail(&id).unwrap().unwrap();
        assert_eq!(detail.steps.len(), 2);
        assert_eq!(detail.actions.len(), 1);
        assert_eq!(detail.links.len(), 2);
        assert_eq!(
            detail.audit[0]["new_hash"],
            detail.experiment["content_hash"]
        );
    }
}

#[test]
fn a_backup_check_is_a_type_of_its_own() {
    let (_d, store, writer) = temp_writer();
    let new = NewManualExperiment {
        exercise_type: ExerciseType::Backup,
        ..restore_test("alice")
    };
    let id = writer.create_manual_draft(&new).unwrap();
    let rows = store
        .read_only()
        .unwrap()
        .manual_experiments(Some("draft"))
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], id.as_str());
    assert_eq!(rows[0]["exercise_type"], "backup");
}

#[test]
fn a_bad_dr_detail_is_refused_before_anything_is_written() {
    let (_d, store, writer) = temp_writer();
    let mut cases: Vec<(NewManualExperiment, &str)> = Vec::new();
    let mut negative = restore_test("alice");
    negative.exercise.rpo_target_s = Some(-1.0);
    cases.push((negative, "'rpo_target_s'"));
    let mut backwards = restore_test("alice");
    backwards.exercise.steps[0].ended_at_ns = Some(99 * DAY);
    cases.push((backwards, "ends before it starts"));
    let mut untitled = restore_test("alice");
    untitled.exercise.steps[1].title = " ".into();
    cases.push((untitled, "step 2"));
    let mut ownerless = restore_test("alice");
    ownerless.exercise.actions[0].owner = String::new();
    cases.push((ownerless, "'owner'"));
    let mut unlinked = restore_test("alice");
    unlinked.exercise.links[1].linked_id = String::new();
    cases.push((unlinked, "'linked_id'"));
    let mut nameless = restore_test("alice");
    nameless.exercise.participants[0].name = String::new();
    cases.push((nameless, "participant"));
    for (new, needle) in cases {
        let err = writer.create_manual_draft(&new).unwrap_err();
        assert!(
            matches!(&err, ManualError::Invalid(msg) if msg.contains(needle)),
            "{needle}: {err:?}"
        );
    }
    let reader = store.read_only().unwrap();
    assert!(reader.manual_experiments(None).unwrap().is_empty());
    let steps = reader
        .query_json_rows("SELECT count(*) AS n FROM manual_exercise_steps")
        .unwrap();
    assert_eq!(steps[0]["n"], 0);
}

#[test]
fn a_record_id_is_bound_not_spliced_into_the_query() {
    let (_d, store, writer) = temp_writer();
    writer.create_manual_draft(&restore_test("alice")).unwrap();
    let reader = store.read_only().unwrap();
    assert!(reader
        .manual_experiment_detail("x' OR '1'='1")
        .unwrap()
        .is_none());
    assert!(reader
        .manual_experiments(Some("draft' OR '1'='1"))
        .unwrap()
        .is_empty());
    assert_eq!(reader.manual_experiments(Some("draft")).unwrap().len(), 1);
}

/// The three manual tables exactly as schema v21 created them, a record
/// in each, and the version set back to 21. The tables of v22 are dropped
/// first, so the lake is what a v21 build left.
const V21_MANUAL: &[&str] = &[
    "DROP TABLE manual_experiments",
    "DROP TABLE manual_experiment_audit",
    "DROP TABLE evidence_attachments",
    "DROP TABLE manual_exercise_steps",
    "DROP TABLE manual_action_items",
    "DROP TABLE manual_experiment_links",
    "DROP TABLE manual_exercise_plans",
    "DROP TABLE manual_plan_audit",
    "DROP TABLE manual_signoffs",
    "CREATE TABLE manual_experiments (
        id VARCHAR NOT NULL,
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
    )",
    "CREATE TABLE manual_experiment_audit (
        id VARCHAR PRIMARY KEY,
        experiment_id VARCHAR NOT NULL,
        changed_by VARCHAR NOT NULL,
        changed_at_ns BIGINT NOT NULL,
        action VARCHAR NOT NULL,
        diff JSON,
        prev_hash VARCHAR,
        new_hash VARCHAR NOT NULL
    )",
    "CREATE INDEX idx_manual_audit_experiment ON manual_experiment_audit (experiment_id)",
    "CREATE TABLE evidence_attachments (
        id VARCHAR PRIMARY KEY,
        experiment_id VARCHAR NOT NULL,
        kind VARCHAR NOT NULL,
        uri VARCHAR NOT NULL,
        label VARCHAR,
        file_hash VARCHAR,
        added_by VARCHAR NOT NULL,
        added_at_ns BIGINT NOT NULL
    )",
    "CREATE INDEX idx_evidence_attachments_experiment ON evidence_attachments (experiment_id)",
    "INSERT INTO manual_experiments (id, experiment_name, exercise_type, executed_at_ns,
        hypothesis, method, outcome_status, action_items, target_system,
        target_environment, recovery_time_s, entered_by, entered_at_ns, attestation,
        status, framework_refs, content_hash)
     VALUES ('m-v21', 'db failover drill', 'drill', 1000, 'h', 'm', 'partial',
        CAST('[\"tune the restore\"]' AS JSON), 'payments-db', 'production', 4100.0,
        'alice', 1000, 'att', 'draft', ['DORA Art. 12(2)'], 'hash-v21')",
    "INSERT INTO manual_experiment_audit VALUES
        ('a-v21', 'm-v21', 'alice', 1000, 'create', '{}', NULL, 'hash-v21'),
        ('a-v21-attach', 'm-v21', 'alice', 1000, 'attach',
         '{\"attachment_id\":\"e-v21\",\"kind\":\"url\",\"uri\":\"https://wiki.example/drill\",\"label\":\"log\"}',
         'hash-v21', 'hash-v21')",
    "INSERT INTO evidence_attachments VALUES
        ('e-v21', 'm-v21', 'url', 'https://wiki.example/drill', 'log', NULL, 'alice', 1000)",
    "UPDATE schema_meta SET value = 21 WHERE key = 'version'",
];

fn columns_of(store: &Store, table: &str) -> Vec<String> {
    store
        .read_only()
        .unwrap()
        .query_json_rows_bound(
            "SELECT column_name FROM information_schema.columns \
             WHERE table_name = ? ORDER BY ordinal_position",
            &[table],
        )
        .unwrap()
        .iter()
        .map(|r| r["column_name"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_v21_lake_gains_the_dr_columns_and_keeps_its_records() {
    let d = tempfile::TempDir::new().unwrap();
    let path = d.path().join("kronika.duckdb");
    {
        let store = Store::open(&path).unwrap();
        let writer = store.writer().unwrap();
        for sql in V21_MANUAL {
            writer.execute(sql, []).unwrap();
        }
        assert_eq!(writer.schema_version().unwrap(), 21);
    }
    // Opening twice proves the migration is idempotent.
    for _ in 0..2 {
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.writer().unwrap().schema_version().unwrap(),
            CURRENT_SCHEMA_VERSION
        );
    }
    let store = Store::open(&path).unwrap();

    let columns = columns_of(&store, "manual_experiments");
    assert_eq!(
        columns[columns.len() - 14..],
        [
            "rto_target_s",
            "rpo_target_s",
            "rpo_measured_s",
            "data_verified",
            "participants",
            "plan_id",
            "hash_version",
            "findings_severity",
            "tester",
            "tester_kind",
            "tlpt_attestation_ref",
            "tlpt_attested_at_ns",
            "tlpt_summary_ref",
            "scenarios"
        ]
    );
    assert!(columns_of(&store, "manual_action_items").ends_with(&[
        "severity".to_string(),
        "retest_kind".to_string(),
        "retest_id".to_string()
    ]));
    assert!(columns_of(&store, "evidence_attachments")
        .ends_with(&["media_type".to_string(), "size_bytes".to_string()]));
    assert!(columns_of(&store, "manual_experiment_audit").ends_with(&["link_hash".to_string()]));
    for table in [
        "manual_exercise_steps",
        "manual_action_items",
        "manual_experiment_links",
        "manual_exercise_plans",
        "manual_plan_audit",
        "manual_signoffs",
    ] {
        assert!(!columns_of(&store, table).is_empty(), "{table} missing");
    }

    // The v21 record reads as it was, with the new fields empty and its
    // seal still the v1 one.
    let detail = store
        .read_only()
        .unwrap()
        .manual_experiment_detail("m-v21")
        .unwrap()
        .unwrap();
    let row = &detail.experiment;
    assert_eq!(row["recovery_time_s"], 4100.0);
    assert_eq!(row["action_items"], json!(["tune the restore"]));
    assert_eq!(row["rto_target_s"], Value::Null);
    assert_eq!(row["rpo_measured_s"], Value::Null);
    assert_eq!(row["participants"], Value::Null);
    assert_eq!(row["hash_version"], 1);
    assert_eq!(row["content_hash"], "hash-v21");
    assert!(detail.steps.is_empty() && detail.actions.is_empty() && detail.links.is_empty());
    assert_eq!(detail.attachments.len(), 1);
    assert_eq!(detail.attachments[0]["media_type"], Value::Null);

    // Its next write re-seals it as v2 and chains the stored v1 hash.
    let writer = store.writer().unwrap();
    writer.submit_manual("m-v21", None, "alice").unwrap();
    writer.verify_manual("m-v21", "bob", None).unwrap();
    drop(writer);
    let detail = store
        .read_only()
        .unwrap()
        .manual_experiment_detail("m-v21")
        .unwrap()
        .unwrap();
    assert_eq!(detail.experiment["status"], "verified");
    assert_eq!(detail.experiment["hash_version"], 2);
    let submit = detail
        .audit
        .iter()
        .find(|a| a["action"] == "submit")
        .unwrap();
    assert_eq!(submit["prev_hash"], "hash-v21");
    assert_ne!(submit["new_hash"], "hash-v21");
    assert_eq!(
        detail.audit.last().unwrap()["new_hash"],
        detail.experiment["content_hash"]
    );
}

/// The version 1 seal of the record `m-v1` below in `status`: SHA-256 of
/// the canonical JSON of 3.0.3, written out by hand.
fn v1_seal(status: &str) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;
    let json = format!(
        "{{\"experiment_name\":\"payments-db drill\",\"exercise_type\":\"drill\",\
         \"executed_at_ns\":1000,\"hypothesis\":\"h\",\"method\":\"m\",\
         \"outcome_status\":\"passed\",\"hypothesis_met\":true,\"findings\":\"f\",\
         \"action_items\":[\"tune\"],\"target_system\":\"payments-db\",\
         \"target_environment\":\"production\",\"blast_radius\":null,\
         \"recovery_time_s\":40.0,\"duration_s\":null,\"entered_by\":\"alice\",\
         \"attestation\":\"att\",\"renewal_due_ns\":null,\
         \"framework_refs\":[\"DORA Art. 12(2)\"],\"status\":\"{status}\"}}"
    );
    Sha256::digest(json.as_bytes())
        .iter()
        .fold(String::new(), |mut hex, b| {
            write!(hex, "{b:02x}").unwrap();
            hex
        })
}

/// A record verified by a v21 build: its seal in version 1 and three audit
/// rows with no link hash.
fn verified_v1_record() -> Vec<String> {
    let (draft, submitted, verified) =
        (v1_seal("draft"), v1_seal("submitted"), v1_seal("verified"));
    vec![
        format!(
            "INSERT INTO manual_experiments (id, experiment_name, exercise_type, executed_at_ns,
                hypothesis, method, outcome_status, hypothesis_met, findings, action_items,
                target_system, target_environment, recovery_time_s, entered_by, entered_at_ns,
                attestation, status, reviewed_by, reviewed_at_ns, framework_refs, content_hash)
             VALUES ('m-v1', 'payments-db drill', 'drill', 1000, 'h', 'm', 'passed', true, 'f',
                CAST('[\"tune\"]' AS JSON), 'payments-db', 'production', 40.0, 'alice', 1000,
                'att', 'verified', 'bob', 3000, ['DORA Art. 12(2)'], '{verified}')"
        ),
        format!(
            "INSERT INTO manual_experiment_audit VALUES
                ('a-v1-1', 'm-v1', 'alice', 1000, 'create', '{{}}', NULL, '{draft}'),
                ('a-v1-2', 'm-v1', 'alice', 2000, 'submit', '{{}}', '{draft}', '{submitted}'),
                ('a-v1-3', 'm-v1', 'bob', 3000, 'verify', '{{}}', '{submitted}', '{verified}')"
        ),
    ]
}

#[test]
fn a_v1_record_of_a_v21_lake_verifies_and_its_audit_rows_are_a_legacy_prefix() {
    let d = tempfile::TempDir::new().unwrap();
    let path = d.path().join("kronika.duckdb");
    {
        let store = Store::open(&path).unwrap();
        let writer = store.writer().unwrap();
        for sql in V21_MANUAL {
            writer.execute(sql, []).unwrap();
        }
        for sql in verified_v1_record() {
            writer.execute(&sql, []).unwrap();
        }
    }
    let store = Store::open(&path).unwrap();
    let verdict = store
        .read_only()
        .unwrap()
        .verify_manual_record("m-v1")
        .unwrap()
        .unwrap();
    assert!(verdict.ok, "{verdict:?}");
    assert_eq!(verdict.hash_version, Some(1));
    assert_eq!(verdict.legacy_rows, 3);
    assert!(verdict.problems.is_empty(), "{verdict:?}");

    // The v21 draft is submitted now: the chain starts after its legacy
    // create and attach rows, and the new row links.
    let writer = store.writer().unwrap();
    writer.submit_manual("m-v21", None, "alice").unwrap();
    let reader = store.read_only().unwrap();
    let verdict = reader.verify_manual_record("m-v21").unwrap().unwrap();
    assert!(verdict.ok, "{verdict:?}");
    assert_eq!((verdict.hash_version, verdict.legacy_rows), (Some(2), 2));
    let audit = reader
        .manual_experiment_detail("m-v21")
        .unwrap()
        .unwrap()
        .audit;
    assert_eq!(audit[0]["link_hash"], Value::Null);
    assert_eq!(audit[1]["link_hash"], Value::Null);
    assert_eq!(audit[2]["link_hash"].as_str().map(str::len), Some(64));

    // The version 1 seal still covers the content, and the audit trail
    // names the reviewer the seal leaves out. A reader opened after each
    // write sees it.
    let now = || {
        store
            .read_only()
            .unwrap()
            .verify_manual_record("m-v1")
            .unwrap()
            .unwrap()
    };
    for (column, value, field) in [
        ("findings", "'f'", None),
        ("reviewed_by", "'bob'", Some("reviewed_by")),
    ] {
        let set =
            |to: &str| format!("UPDATE manual_experiments SET {column} = {to} WHERE id = 'm-v1'");
        writer.execute(&set("'mallory'"), []).unwrap();
        let verdict = now();
        writer.execute(&set(value), []).unwrap();
        assert!(!verdict.ok, "{column}");
        assert!(
            verdict
                .problems
                .iter()
                .any(|p| p.table == "manual_experiments"
                    && p.row == "m-v1"
                    && p.field.as_deref() == field),
            "{column}: {verdict:?}"
        );
    }
    let verdict = now();
    assert!(verdict.ok, "{verdict:?}");

    // A legacy row has no link hash, but the seal chain still shows it
    // was deleted.
    writer
        .execute(
            "DELETE FROM manual_experiment_audit WHERE id = 'a-v1-2'",
            [],
        )
        .unwrap();
    let verdict = now();
    assert!(!verdict.ok);
    assert!(
        verdict
            .problems
            .iter()
            .any(|p| p.table == "manual_experiment_audit"
                && p.row == "a-v1-3"
                && p.field.as_deref() == Some("prev_hash")),
        "{verdict:?}"
    );
}

#[test]
fn the_seal_problem_says_what_the_seal_of_its_version_covers() {
    let d = tempfile::TempDir::new().unwrap();
    let path = d.path().join("kronika.duckdb");
    {
        let store = Store::open(&path).unwrap();
        let writer = store.writer().unwrap();
        for sql in V21_MANUAL {
            writer.execute(sql, []).unwrap();
        }
        for sql in verified_v1_record() {
            writer.execute(&sql, []).unwrap();
        }
    }
    let store = Store::open(&path).unwrap();
    let writer = store.writer().unwrap();
    let v2 = writer.create_manual_draft(&restore_test("alice")).unwrap();
    writer
        .execute(
            "UPDATE manual_experiments SET findings = 'changed later'",
            [],
        )
        .unwrap();

    let reader = store.read_only().unwrap();
    let says = |id: &str| {
        let verdict = reader.verify_manual_record(id).unwrap().unwrap();
        verdict
            .problems
            .into_iter()
            .find(|p| p.table == "manual_experiments" && p.field.is_none())
            .unwrap_or_else(|| panic!("no seal problem for {id}"))
            .problem
    };
    assert_eq!(
        says("m-v1"),
        "the stored seal is not the seal of the record: a column of the record was \
         changed after it was sealed (a version 1 seal does not cover the reviewer, the \
         steps, the actions or the links)"
    );
    assert_eq!(
        says(&v2),
        "the stored seal is not the seal of the record: a column of the record, a step, an \
         action or a link was changed after it was sealed"
    );
}
