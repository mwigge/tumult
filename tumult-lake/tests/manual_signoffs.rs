//! Sign-offs of a manual record and the audit verify: a record needs as
//! many approvals as its type requires, each sign-off is a row chained to
//! the one before it, every new audit row links the row before it, and
//! the verify recomputes all of it from the lake.

#![cfg(feature = "duckdb")]

use serde_json::Value;
use tumult_lake::{
    ExerciseDetail, ExerciseType, ManualError, ManualOutcome, NewExercisePlan, NewManualExperiment,
    PlanEdit, SignOff, SignOffDecision, Store, Writer,
};

const DAY: i64 = 86_400 * 1_000_000_000;

fn temp_writer() -> (tempfile::TempDir, Store, Writer) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    (d, store, writer)
}

fn failover(by: &str) -> NewManualExperiment {
    NewManualExperiment {
        experiment_name: "payments-db failover".into(),
        exercise_type: ExerciseType::Failover,
        executed_at_ns: 95 * DAY,
        hypothesis: "The standby takes the writes within 900 s".into(),
        method: "Stop the primary and promote the standby".into(),
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

/// Two approvals for a failover, one for every other type.
fn two_for_a_failover(exercise_type: &str) -> u32 {
    if exercise_type == "failover" {
        2
    } else {
        1
    }
}

fn approve(by: &str) -> SignOff<'_> {
    SignOff {
        by,
        note: Some("logs checked"),
        decision: SignOffDecision::Approved,
    }
}

#[test]
fn a_record_needs_as_many_approvals_from_distinct_users_as_its_type_requires() {
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&failover("alice")).unwrap();
    writer.submit_manual(&id, None, "alice").unwrap();

    let first = writer
        .sign_off_manual(&id, &approve("bob"), two_for_a_failover)
        .unwrap();
    assert_eq!(
        (first.status.as_str(), first.approvals, first.required),
        ("submitted", 1, 2)
    );
    let err = writer
        .sign_off_manual(&id, &approve("bob"), two_for_a_failover)
        .unwrap_err();
    assert!(
        matches!(&err, ManualError::AlreadySigned(by) if by == "bob"),
        "{err:?}"
    );
    assert_eq!(err.to_string(), "'bob' has already signed off this record");
    let err = writer
        .sign_off_manual(&id, &approve("alice"), two_for_a_failover)
        .unwrap_err();
    assert!(matches!(err, ManualError::SelfReview), "{err:?}");

    let second = writer
        .sign_off_manual(&id, &approve("carol"), two_for_a_failover)
        .unwrap();
    assert_eq!(
        (second.status.as_str(), second.approvals, second.required),
        ("verified", 2, 2)
    );
    drop(writer);

    let reader = store.read_only().unwrap();
    let detail = reader.manual_experiment_detail(&id).unwrap().unwrap();
    assert_eq!(detail.experiment["reviewed_by"], "carol");
    let signoffs = &detail.signoffs;
    assert_eq!(signoffs.len(), 2);
    assert_eq!(signoffs[0]["prev_hash"], Value::Null);
    assert_eq!(signoffs[1]["prev_hash"], signoffs[0]["link_hash"]);
    let submitted_seal = &detail.audit[1]["new_hash"];
    for s in signoffs {
        assert_eq!(&s["signed_hash"], submitted_seal);
        assert_eq!(s["subject_kind"], "record");
        assert_eq!(s["role"], "reviewer");
        assert_eq!(s["link_hash"].as_str().map(str::len), Some(64));
    }
    let verdict = reader.verify_manual_record(&id).unwrap().unwrap();
    assert!(verdict.ok, "{verdict:?}");
    assert_eq!(verdict.legacy_rows, 0);
}

#[test]
fn every_new_audit_row_of_a_record_and_a_plan_links_the_row_before_it() {
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&failover("alice")).unwrap();
    writer.submit_manual(&id, None, "alice").unwrap();
    writer.verify_manual(&id, "bob", None).unwrap();
    let plan = NewExercisePlan {
        name: "payments-db failover".into(),
        exercise_type: ExerciseType::Failover,
        target_system: None,
        target_environment: Some("production".into()),
        function_id: None,
        scope: None,
        owner: "alice".into(),
        planned_at_ns: None,
        due_at_ns: 100 * DAY,
        interval_days: None,
        rto_target_s: Some(900.0),
        rpo_target_s: None,
    };
    let plan = writer.create_manual_plan(&plan, "alice").unwrap();
    let edit = PlanEdit {
        rto_target_s: Some(Some(600.25)),
        ..PlanEdit::default()
    };
    writer.edit_manual_plan(&plan, &edit, "bob").unwrap();
    drop(writer);

    let reader = store.read_only().unwrap();
    let record = reader.manual_experiment_detail(&id).unwrap().unwrap();
    let plan_detail = reader.manual_plan_detail(&plan).unwrap().unwrap();
    for audit in [&record.audit, &plan_detail.audit] {
        let links: Vec<&str> = audit
            .iter()
            .map(|a| a["link_hash"].as_str().unwrap())
            .collect();
        assert!(links.iter().all(|l| l.len() == 64), "{audit:?}");
        let distinct: std::collections::BTreeSet<&&str> = links.iter().collect();
        assert_eq!(distinct.len(), links.len(), "{audit:?}");
    }
    let verdict = reader.verify_manual_record(&id).unwrap().unwrap();
    assert!(verdict.ok, "{verdict:?}");
    assert_eq!(verdict.hash_version, Some(2));
    let verdict = reader.verify_manual_plan(&plan).unwrap().unwrap();
    assert!(verdict.ok, "{verdict:?}");
    assert_eq!(verdict.hash_version, None);
    assert!(reader
        .verify_manual_record("no-such-record")
        .unwrap()
        .is_none());
    assert!(reader.verify_manual_plan("no-such-plan").unwrap().is_none());
}

#[test]
fn verify_names_the_reviewer_of_a_rejected_record_that_was_changed() {
    let (_d, store, writer) = temp_writer();
    let id = writer.create_manual_draft(&failover("alice")).unwrap();
    writer.submit_manual(&id, None, "alice").unwrap();
    writer
        .reject_manual(&id, "bob", "the promotion log is missing")
        .unwrap();
    let verdict = || {
        store
            .read_only()
            .unwrap()
            .verify_manual_record(&id)
            .unwrap()
            .unwrap()
    };
    assert!(verdict().ok, "{:?}", verdict());

    writer
        .execute(
            &format!("UPDATE manual_experiments SET reviewed_by = 'mallory' WHERE id = '{id}'"),
            [],
        )
        .unwrap();
    let changed = verdict();
    assert!(!changed.ok);
    let problem = changed
        .problems
        .iter()
        .find(|p| p.field.as_deref() == Some("reviewed_by"))
        .unwrap_or_else(|| panic!("no reviewed_by problem: {changed:?}"));
    assert_eq!(problem.row, id);
    assert_eq!(
        problem.problem,
        "reviewed_by is 'mallory', but the reject row of the audit trail is by 'bob'"
    );
}

#[test]
fn a_user_who_submitted_or_attached_to_a_record_may_not_sign_it_off() {
    let (_d, _store, writer) = temp_writer();
    let id = writer.create_manual_draft(&failover("alice")).unwrap();
    writer
        .add_manual_attachment(
            &id,
            tumult_lake::AttachmentKind::Url,
            "https://wiki.example/failover",
            None,
            None,
            "dan",
        )
        .unwrap();
    writer.submit_manual(&id, None, "carol").unwrap();
    for by in ["carol", "dan"] {
        let err = writer
            .sign_off_manual(&id, &approve(by), two_for_a_failover)
            .unwrap_err();
        assert!(matches!(err, ManualError::SelfReview), "{by}: {err:?}");
    }
    let outcome = writer
        .sign_off_manual(&id, &approve("bob"), two_for_a_failover)
        .unwrap();
    assert_eq!(outcome.approvals, 1);
}
