//! The DORA fields of a manual record, from the gaps of the DORA report:
//! requirement ids checked on write against the registry, a severity on
//! the findings and on each action, the TLPT block of a `tlpt` record with
//! its remediation plan and retests, and the tester and its kind.

#![cfg(feature = "duckdb")]

use serde_json::Value;
use tumult_lake::{
    ActionItem, ExerciseDetail, ExerciseType, ManualError, ManualOutcome, NewManualExperiment,
    Retest, RetestKind, Severity, Store, Tester, TesterKind, TlptDetail, Writer,
};

const DAY: i64 = 86_400 * 1_000_000_000;

fn temp_writer() -> (tempfile::TempDir, Store, Writer) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    (d, store, writer)
}

fn record(exercise_type: ExerciseType, refs: &[&str]) -> NewManualExperiment {
    NewManualExperiment {
        experiment_name: "payments TLPT".into(),
        exercise_type,
        executed_at_ns: 100 * DAY,
        hypothesis: "A red team cannot reach the ledger".into(),
        method: "Threat-led test of the payments chain".into(),
        outcome: ManualOutcome::Partial,
        hypothesis_met: Some(false),
        findings: Some("The red team reached the ledger by a stale VPN account".into()),
        action_items: vec![],
        target_system: Some("payments".into()),
        target_environment: Some("production".into()),
        blast_radius: None,
        recovery_time_s: None,
        duration_s: None,
        entered_by: "alice".into(),
        attestation: "I attest this record reflects the test as executed.".into(),
        renewal_due_ns: None,
        framework_refs: refs.iter().map(|r| (*r).to_owned()).collect(),
        exercise: ExerciseDetail::default(),
    }
}

fn action(title: &str, severity: Option<Severity>, retest: Option<Retest>) -> ActionItem {
    ActionItem {
        title: title.into(),
        owner: "carol".into(),
        due_at_ns: Some(130 * DAY),
        ticket: None,
        severity,
        retest,
    }
}

fn tlpt_record() -> NewManualExperiment {
    let mut new = record(ExerciseType::Tlpt, &["dora.art26.1"]);
    new.exercise.findings_severity = Some(Severity::High);
    new.exercise.tester = Some(Tester {
        name: "Red Team Ltd".into(),
        kind: TesterKind::External,
    });
    new.exercise.tlpt = Some(TlptDetail {
        attestation_ref: Some("TLPT-ATT-2026-07".into()),
        attested_at_ns: Some(120 * DAY),
        summary_report_ref: Some("TSR-2026-07".into()),
    });
    new.exercise.actions = vec![
        action(
            "Remove stale VPN accounts",
            Some(Severity::Critical),
            Some(Retest {
                kind: RetestKind::Run,
                id: "exp-vpn-retest".into(),
            }),
        ),
        action(
            "Alert on a ledger login from the VPN",
            Some(Severity::Medium),
            None,
        ),
    ];
    new
}

fn detail(store: &Store, id: &str) -> tumult_lake::ManualDetail {
    store
        .read_only()
        .unwrap()
        .manual_experiment_detail(id)
        .unwrap()
        .unwrap()
}

fn count(store: &Store, table: &str) -> i64 {
    store
        .read_only()
        .unwrap()
        .query_json_rows(&format!("SELECT count(*) AS n FROM {table}"))
        .unwrap()[0]["n"]
        .as_i64()
        .unwrap()
}

#[test]
fn an_unknown_requirement_id_is_refused_with_the_nearest_registry_id() {
    let (_d, store, writer) = temp_writer();
    let err = writer
        .create_manual_draft(&record(
            ExerciseType::Drill,
            &["dora.art12.1", "dora.art24.7"],
        ))
        .unwrap_err();
    let ManualError::UnknownRequirement(unknown) = &err else {
        panic!("{err:?}");
    };
    assert_eq!(unknown.reference, "dora.art24.7");
    assert_eq!(unknown.suggestion.as_deref(), Some("dora.art24.1"));
    assert_eq!(
        err.to_string(),
        "framework_refs: 'dora.art24.7' is not a requirement id of the DORA registry; \
         did you mean 'dora.art24.1'?"
    );
    assert_eq!(count(&store, "manual_experiments"), 0);

    let id = writer
        .create_manual_draft(&record(ExerciseType::Drill, &["dora.art12.1"]))
        .unwrap();
    let typo = record(ExerciseType::Drill, &["Art. 12(1)"]);
    let err = writer.update_manual_draft(&id, &typo, "alice").unwrap_err();
    assert!(
        matches!(&err, ManualError::UnknownRequirement(u)
            if u.suggestion.as_deref() == Some("dora.art12.1")),
        "{err:?}"
    );
    let batch = [
        record(ExerciseType::Drill, &["dora.art12.1"]),
        record(ExerciseType::Drill, &["DORA Art. 99"]),
    ];
    let err = writer
        .import_manual_drafts(&batch, Some("q3".into()))
        .unwrap_err();
    assert!(matches!(err, ManualError::UnknownRequirement(_)), "{err:?}");
    assert_eq!(count(&store, "manual_experiments"), 1);
}

#[test]
fn a_known_requirement_in_any_form_and_another_framework_are_kept_as_written() {
    let (_d, store, writer) = temp_writer();
    let refs = [
        "dora.art12.1",
        "DORA Art. 24(6)",
        "RTS 2024/1774 Art. 25",
        "ISO 27001 A.5.30",
    ];
    let id = writer
        .create_manual_draft(&record(ExerciseType::Drill, &refs))
        .unwrap();
    assert_eq!(
        detail(&store, &id).experiment["framework_refs"],
        serde_json::json!(refs)
    );
}

#[test]
fn a_tlpt_record_keeps_its_attestation_summary_report_and_remediation_plan() {
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&tlpt_record()).unwrap();
    let detail = detail(&store, &id);
    let row = &detail.experiment;
    assert_eq!(row["findings_severity"], "high");
    assert_eq!(row["tester"], "Red Team Ltd");
    assert_eq!(row["tester_kind"], "external");
    assert_eq!(row["tlpt_attestation_ref"], "TLPT-ATT-2026-07");
    assert_eq!(row["tlpt_attested_at_ns"], 120 * DAY);
    assert_eq!(row["tlpt_summary_ref"], "TSR-2026-07");
    let plan: Vec<(&str, &Value, &Value, &Value)> = detail
        .actions
        .iter()
        .map(|a| {
            (
                a["title"].as_str().unwrap(),
                &a["severity"],
                &a["retest_kind"],
                &a["retest_id"],
            )
        })
        .collect();
    assert_eq!(
        plan,
        [
            (
                "Remove stale VPN accounts",
                &Value::from("critical"),
                &Value::from("run"),
                &Value::from("exp-vpn-retest")
            ),
            (
                "Alert on a ledger login from the VPN",
                &Value::from("medium"),
                &Value::Null,
                &Value::Null
            ),
        ]
    );
}

#[test]
fn a_draft_edit_replaces_the_severity_the_tester_and_the_tlpt_block() {
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&tlpt_record()).unwrap();
    let mut edited = tlpt_record();
    edited.exercise.findings_severity = None;
    edited.exercise.tester = Some(Tester {
        name: "internal red team".into(),
        kind: TesterKind::Internal,
    });
    edited.exercise.tlpt = None;
    edited.exercise.actions = vec![action(
        "Remove stale VPN accounts",
        Some(Severity::Low),
        Some(Retest {
            kind: RetestKind::Record,
            id: "m-retest".into(),
        }),
    )];
    writer.update_manual_draft(&id, &edited, "alice").unwrap();
    let detail = detail(&store, &id);
    let row = &detail.experiment;
    assert_eq!(row["findings_severity"], Value::Null);
    assert_eq!(row["tester_kind"], "internal");
    assert_eq!(row["tlpt_attestation_ref"], Value::Null);
    assert_eq!(row["tlpt_attested_at_ns"], Value::Null);
    assert_eq!(detail.actions.len(), 1);
    assert_eq!(detail.actions[0]["severity"], "low");
    assert_eq!(detail.actions[0]["retest_kind"], "record");
    assert_eq!(detail.actions[0]["retest_id"], "m-retest");
}

#[test]
fn a_tlpt_block_on_another_exercise_type_is_refused() {
    let (_d, store, writer) = temp_writer();
    let mut pentest = tlpt_record();
    pentest.exercise_type = ExerciseType::Pentest;
    let err = writer.create_manual_draft(&pentest).unwrap_err();
    assert!(
        matches!(&err, ManualError::Invalid(msg)
            if msg == "the tlpt block is for a record of exercise_type 'tlpt' only"),
        "{err:?}"
    );
    pentest.exercise.tlpt = None;
    writer.create_manual_draft(&pentest).unwrap();
    assert_eq!(count(&store, "manual_experiments"), 1);
}

#[test]
fn a_blank_tester_retest_or_tlpt_reference_is_refused() {
    let (_d, store, writer) = temp_writer();
    let mut blank_tester = tlpt_record();
    blank_tester.exercise.tester = Some(Tester {
        name: " ".into(),
        kind: TesterKind::Internal,
    });
    let mut blank_retest = tlpt_record();
    blank_retest.exercise.actions[0].retest = Some(Retest {
        kind: RetestKind::Run,
        id: String::new(),
    });
    let mut blank_ref = tlpt_record();
    blank_ref.exercise.tlpt = Some(TlptDetail {
        summary_report_ref: Some(String::new()),
        ..TlptDetail::default()
    });
    let mut epoch = tlpt_record();
    epoch.exercise.tlpt = Some(TlptDetail {
        attested_at_ns: Some(0),
        ..TlptDetail::default()
    });
    for (new, reason) in [
        (blank_tester, "'tester.name' must not be empty"),
        (blank_retest, "action 1: 'retest id' must not be empty"),
        (blank_ref, "tlpt 'summary_report_ref' must not be empty"),
        (
            epoch,
            "tlpt 'attested_at_ns' must be a positive unix-nanos timestamp",
        ),
    ] {
        let err = writer.create_manual_draft(&new).unwrap_err();
        assert!(
            matches!(&err, ManualError::Invalid(msg) if msg == reason),
            "{err:?}"
        );
    }
    assert_eq!(count(&store, "manual_experiments"), 0);
}

#[test]
fn the_seal_covers_the_dora_fields_and_the_verify_finds_a_direct_edit() {
    let (_d, store, writer) = temp_writer();
    let verified = || {
        let id = writer.create_manual_draft(&tlpt_record()).unwrap();
        writer.submit_manual(&id, None, "alice").unwrap();
        writer.verify_manual(&id, "bob", None).unwrap();
        id
    };
    let ok = |id: &str| {
        store
            .read_only()
            .unwrap()
            .verify_manual_record(id)
            .unwrap()
            .unwrap()
            .ok
    };
    for sql in [
        "UPDATE manual_experiments SET tester_kind = 'internal' WHERE id = ?",
        "UPDATE manual_experiments SET tlpt_summary_ref = 'TSR-other' WHERE id = ?",
        "UPDATE manual_experiments SET findings_severity = 'low' WHERE id = ?",
        "UPDATE manual_action_items SET severity = 'low' WHERE experiment_id = ?",
        "UPDATE manual_action_items SET retest_id = 'exp-other' WHERE experiment_id = ?",
    ] {
        let id = verified();
        assert!(ok(&id));
        writer.execute(sql, [&id]).unwrap();
        assert!(!ok(&id), "not sealed: {sql}");
    }
}

#[test]
fn a_v22_lake_written_before_the_dora_fields_gains_them_on_open() {
    let d = tempfile::TempDir::new().unwrap();
    let path = d.path().join("kronika.duckdb");
    {
        let store = Store::open(&path).unwrap();
        let writer = store.writer().unwrap();
        for column in [
            "findings_severity",
            "tester",
            "tester_kind",
            "tlpt_attestation_ref",
            "tlpt_attested_at_ns",
            "tlpt_summary_ref",
        ] {
            writer
                .execute(
                    &format!("ALTER TABLE manual_experiments DROP COLUMN {column}"),
                    [],
                )
                .unwrap();
        }
        for column in ["severity", "retest_kind", "retest_id"] {
            writer
                .execute(
                    &format!("ALTER TABLE manual_action_items DROP COLUMN {column}"),
                    [],
                )
                .unwrap();
        }
    }
    let store = Store::open(&path).unwrap();
    let id = store
        .writer()
        .unwrap()
        .create_manual_draft(&tlpt_record())
        .unwrap();
    let detail = detail(&store, &id);
    assert_eq!(detail.experiment["tester"], "Red Team Ltd");
    assert_eq!(detail.actions[0]["retest_id"], "exp-vpn-retest");
}
