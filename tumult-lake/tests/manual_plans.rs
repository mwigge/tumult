//! Schema v22 planned exercises: a plan can be edited while it is
//! planned, every write seals it and adds one audit row, a verified record
//! fulfils it, and a plan with an interval comes back after the exercise.

#![cfg(feature = "duckdb")]

use std::collections::BTreeSet;

use serde_json::{json, Value};
use tumult_lake::lake::{committed_file, export, LakeConfig};
use tumult_lake::{
    ExerciseDetail, ExerciseLink, ExerciseType, LinkKind, ManualError, ManualOutcome,
    NewExercisePlan, NewManualExperiment, PlanDetail, PlanEdit, PlanError, PlanFilter, Reader,
    Store, Writer,
};

const DAY: i64 = 86_400 * 1_000_000_000;

fn temp_writer() -> (tempfile::TempDir, Store, Writer) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    let writer = store.writer().unwrap();
    (d, store, writer)
}

/// A yearly failover of the payments database, due on day 100.
fn failover_plan() -> NewExercisePlan {
    NewExercisePlan {
        name: "payments-db failover".into(),
        exercise_type: ExerciseType::Failover,
        target_system: Some("payments-db".into()),
        target_environment: Some("production".into()),
        function_id: Some("cif-0001".into()),
        scope: Some("Promote the standby and move the writers".into()),
        owner: "alice".into(),
        planned_at_ns: Some(90 * DAY),
        due_at_ns: 100 * DAY,
        interval_days: Some(365),
        rto_target_s: Some(900.0),
        rpo_target_s: Some(60.0),
    }
}

/// The failover as it ran on day 95 against the function `cif-0001`,
/// naming `plan_id`.
fn failover_record(plan_id: Option<&str>) -> NewManualExperiment {
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
        entered_by: "carol".into(),
        attestation: "I attest this record reflects the exercise as executed.".into(),
        renewal_due_ns: None,
        framework_refs: vec![],
        exercise: ExerciseDetail {
            links: vec![ExerciseLink {
                kind: LinkKind::Function,
                linked_id: "cif-0001".into(),
                label: None,
            }],
            plan_id: plan_id.map(str::to_owned),
            ..ExerciseDetail::default()
        },
    }
}

fn plan_of(store: &Store, id: &str) -> PlanDetail {
    store
        .read_only()
        .unwrap()
        .manual_plan_detail(id)
        .unwrap()
        .unwrap()
}

fn actions(detail: &PlanDetail) -> Vec<&str> {
    detail
        .audit
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect()
}

/// Each audit row starts from the seal the row before it left, and the
/// last one left the seal the plan holds now.
fn assert_chained(detail: &PlanDetail) {
    assert_eq!(detail.audit[0]["prev_hash"], Value::Null);
    for w in detail.audit.windows(2) {
        assert_eq!(w[1]["prev_hash"], w[0]["new_hash"], "{w:?}");
        assert_ne!(w[1]["prev_hash"], w[1]["new_hash"], "{w:?}");
    }
    assert_eq!(
        detail.audit.last().unwrap()["new_hash"],
        detail.plan["content_hash"]
    );
}

fn all_plans(store: &Store) -> Vec<Value> {
    store
        .read_only()
        .unwrap()
        .manual_plans(&PlanFilter::default())
        .unwrap()
}

#[test]
fn a_plan_is_created_edited_and_cancelled_with_one_sealed_audit_row_per_write() {
    let (_d, store, writer) = temp_writer();
    let id = writer
        .create_manual_plan(&failover_plan(), "alice")
        .unwrap();

    let created = plan_of(&store, &id);
    let row = &created.plan;
    assert_eq!(row["status"], "planned");
    assert_eq!(row["name"], "payments-db failover");
    assert_eq!(row["exercise_type"], "failover");
    assert_eq!(row["target_system"], "payments-db");
    assert_eq!(row["target_environment"], "production");
    assert_eq!(row["function_id"], "cif-0001");
    assert_eq!(row["owner"], "alice");
    assert_eq!(row["planned_at_ns"], 90 * DAY);
    assert_eq!(row["due_at_ns"], 100 * DAY);
    assert_eq!(row["interval_days"], 365);
    assert_eq!(row["rto_target_s"], 900.0);
    assert_eq!(row["rpo_target_s"], 60.0);
    assert_eq!(row["created_by"], "alice");
    assert_eq!(row["updated_by"], Value::Null);
    assert_eq!(actions(&created), ["create"]);
    assert_chained(&created);

    let edit = PlanEdit {
        owner: Some("carol".into()),
        due_at_ns: Some(120 * DAY),
        scope: Some(None),
        ..PlanEdit::default()
    };
    writer.edit_manual_plan(&id, &edit, "bob").unwrap();
    let edited = plan_of(&store, &id);
    assert_eq!(edited.plan["owner"], "carol");
    assert_eq!(edited.plan["due_at_ns"], 120 * DAY);
    assert_eq!(edited.plan["scope"], Value::Null);
    assert_eq!(
        edited.plan["planned_at_ns"],
        90 * DAY,
        "kept: not in the edit"
    );
    assert_eq!(edited.plan["updated_by"], "bob");
    assert_eq!(actions(&edited), ["create", "edit"]);
    assert_eq!(
        edited.audit[1]["diff"],
        json!({"fields": {"owner": "carol", "due_at_ns": 120 * DAY, "scope": null}})
    );
    assert_chained(&edited);

    writer
        .cancel_manual_plan(&id, "carol", Some("moved to the Q1 window"))
        .unwrap();
    let cancelled = plan_of(&store, &id);
    assert_eq!(cancelled.plan["status"], "cancelled");
    assert_eq!(cancelled.plan["updated_by"], "carol");
    assert_eq!(actions(&cancelled), ["create", "edit", "cancel"]);
    assert_eq!(
        cancelled.audit[2]["diff"]["reason"],
        "moved to the Q1 window"
    );
    assert_chained(&cancelled);

    // A cancelled plan is closed: no edit, no second cancel, no new row.
    let err = writer.edit_manual_plan(&id, &edit, "bob").unwrap_err();
    assert!(
        matches!(&err, ManualError::Plan(PlanError::WrongStatus { status, action })
            if status == "cancelled" && action == "edit"),
        "{err:?}"
    );
    let err = writer.cancel_manual_plan(&id, "bob", None).unwrap_err();
    assert!(
        matches!(&err, ManualError::Plan(PlanError::WrongStatus { .. })),
        "{err:?}"
    );
    assert_eq!(plan_of(&store, &id).audit.len(), 3);
}

#[test]
fn a_verified_record_fulfils_its_plan_and_the_next_plan_is_due_one_interval_after_it_ran() {
    let (_d, store, writer) = temp_writer();
    let plan = writer
        .create_manual_plan(&failover_plan(), "alice")
        .unwrap();
    let record = writer
        .create_manual_draft(&failover_record(Some(&plan)))
        .unwrap();
    writer.submit_manual(&record, None, "carol").unwrap();
    assert_eq!(
        plan_of(&store, &plan).plan["status"],
        "planned",
        "a record fulfils its plan when it is verified, not before"
    );

    writer.verify_manual(&record, "bob", None).unwrap();

    let fulfilled = plan_of(&store, &plan);
    assert_eq!(fulfilled.plan["status"], "fulfilled");
    assert_eq!(fulfilled.plan["updated_by"], "bob");
    assert_eq!(actions(&fulfilled), ["create", "fulfil"]);
    assert_chained(&fulfilled);
    let diff = &fulfilled.audit[1]["diff"];
    assert_eq!(diff["record_id"], record.as_str());

    let plans = all_plans(&store);
    assert_eq!(plans.len(), 2);
    let next = plans.iter().find(|p| p["id"] != plan.as_str()).unwrap();
    assert_eq!(diff["next_plan_id"], next["id"]);
    assert_eq!(next["status"], "planned");
    assert_eq!(next["due_at_ns"], 95 * DAY + 365 * DAY);
    assert_eq!(next["planned_at_ns"], Value::Null);
    for kept in [
        "name",
        "exercise_type",
        "target_system",
        "target_environment",
        "function_id",
        "scope",
        "owner",
        "interval_days",
        "rto_target_s",
        "rpo_target_s",
    ] {
        assert_eq!(next[kept], fulfilled.plan[kept], "{kept}");
    }
    let next = plan_of(&store, next["id"].as_str().unwrap());
    assert_eq!(actions(&next), ["create"]);
    assert_eq!(next.audit[0]["diff"]["previous_plan_id"], plan.as_str());
    assert_chained(&next);

    // A fulfilled plan is closed.
    let edit = PlanEdit {
        name: Some("renamed".into()),
        ..PlanEdit::default()
    };
    let err = writer.edit_manual_plan(&plan, &edit, "alice").unwrap_err();
    assert!(
        matches!(&err, ManualError::Plan(PlanError::WrongStatus { status, .. }) if status == "fulfilled"),
        "{err:?}"
    );
    let err = writer.cancel_manual_plan(&plan, "alice", None).unwrap_err();
    assert!(
        matches!(&err, ManualError::Plan(PlanError::WrongStatus { .. })),
        "{err:?}"
    );
}

#[test]
fn a_rejected_record_leaves_its_plan_planned() {
    let (_d, store, writer) = temp_writer();
    let plan = writer
        .create_manual_plan(&failover_plan(), "alice")
        .unwrap();
    let record = writer
        .create_manual_draft(&failover_record(Some(&plan)))
        .unwrap();
    writer.submit_manual(&record, None, "carol").unwrap();
    writer
        .reject_manual(&record, "bob", "the promotion log is missing")
        .unwrap();

    let detail = plan_of(&store, &plan);
    assert_eq!(detail.plan["status"], "planned");
    assert_eq!(actions(&detail), ["create"]);
    assert_eq!(all_plans(&store).len(), 1);
}

#[test]
fn a_plan_without_an_interval_has_no_next_plan() {
    let (_d, store, writer) = temp_writer();
    let once = NewExercisePlan {
        interval_days: None,
        ..failover_plan()
    };
    let plan = writer.create_manual_plan(&once, "alice").unwrap();
    let record = writer
        .create_manual_draft(&failover_record(Some(&plan)))
        .unwrap();
    writer.submit_manual(&record, None, "carol").unwrap();
    writer.verify_manual(&record, "bob", None).unwrap();

    let detail = plan_of(&store, &plan);
    assert_eq!(detail.plan["status"], "fulfilled");
    assert_eq!(detail.audit[1]["diff"]["next_plan_id"], Value::Null);
    assert_eq!(all_plans(&store).len(), 1);
}

fn assert_unusable(err: &ManualError, needle: &str) {
    assert!(
        matches!(err, ManualError::Plan(PlanError::Unusable(msg)) if msg.contains(needle)),
        "{err:?} should say {needle}"
    );
}

#[test]
fn a_record_names_only_a_planned_plan_of_its_own_type() {
    let (_d, store, writer) = temp_writer();
    let plan = writer
        .create_manual_plan(&failover_plan(), "alice")
        .unwrap();
    let cancelled = writer
        .create_manual_plan(&failover_plan(), "alice")
        .unwrap();
    writer
        .cancel_manual_plan(&cancelled, "alice", None)
        .unwrap();

    let err = writer
        .create_manual_draft(&failover_record(Some("no-such-plan")))
        .unwrap_err();
    assert_unusable(&err, "plan 'no-such-plan' does not exist");
    let restore = NewManualExperiment {
        exercise_type: ExerciseType::Restore,
        ..failover_record(Some(&plan))
    };
    let err = writer.create_manual_draft(&restore).unwrap_err();
    assert_unusable(&err, "plans a failover exercise, not a restore");
    let err = writer
        .create_manual_draft(&failover_record(Some(&cancelled)))
        .unwrap_err();
    assert_unusable(&err, "is cancelled, not planned");
    let err = writer
        .import_manual_drafts(&[failover_record(Some("no-such-plan"))], None)
        .unwrap_err();
    assert_unusable(&err, "does not exist");
    assert!(store
        .read_only()
        .unwrap()
        .manual_experiments(None)
        .unwrap()
        .is_empty());

    // A draft may not move to a plan it cannot fulfil either.
    let record = writer.create_manual_draft(&failover_record(None)).unwrap();
    let err = writer
        .update_manual_draft(&record, &failover_record(Some(&cancelled)), "carol")
        .unwrap_err();
    assert_unusable(&err, "is cancelled");
    writer
        .update_manual_draft(&record, &failover_record(Some(&plan)), "carol")
        .unwrap();

    // The plan is checked again at sign-off: it was cancelled meanwhile, so
    // the record stays submitted and the plan stays cancelled.
    writer.submit_manual(&record, None, "carol").unwrap();
    writer.cancel_manual_plan(&plan, "alice", None).unwrap();
    let err = writer.verify_manual(&record, "bob", None).unwrap_err();
    assert_unusable(&err, "is cancelled");
    let detail = store
        .read_only()
        .unwrap()
        .manual_experiment_detail(&record)
        .unwrap()
        .unwrap();
    assert_eq!(detail.experiment["status"], "submitted");
    assert_eq!(detail.experiment["plan_id"], plan.as_str());
    assert_eq!(plan_of(&store, &plan).plan["status"], "cancelled");
}

#[test]
fn a_record_must_test_the_target_environment_and_function_its_plan_planned() {
    let (_d, store, writer) = temp_writer();
    let plan = writer
        .create_manual_plan(&failover_plan(), "alice")
        .unwrap();

    let staging = NewManualExperiment {
        target_environment: Some("staging".into()),
        ..failover_record(Some(&plan))
    };
    let err = writer.create_manual_draft(&staging).unwrap_err();
    assert_unusable(
        &err,
        "plans target_environment 'production', the record has 'staging'",
    );
    let other = NewManualExperiment {
        target_system: Some("ledger-db".into()),
        ..failover_record(Some(&plan))
    };
    let err = writer.create_manual_draft(&other).unwrap_err();
    assert_unusable(
        &err,
        "plans target_system 'payments-db', the record has 'ledger-db'",
    );
    let unnamed = NewManualExperiment {
        target_system: None,
        ..failover_record(Some(&plan))
    };
    let err = writer.create_manual_draft(&unnamed).unwrap_err();
    assert_unusable(
        &err,
        "plans target_system 'payments-db', the record has none",
    );
    let mut unlinked = failover_record(Some(&plan));
    unlinked.exercise.links.clear();
    let err = writer.create_manual_draft(&unlinked).unwrap_err();
    assert_unusable(
        &err,
        "plans function_id 'cif-0001', the record links no such function",
    );
    let err = writer
        .import_manual_drafts(std::slice::from_ref(&staging), None)
        .unwrap_err();
    assert_unusable(&err, "plans target_environment 'production'");
    assert!(store
        .read_only()
        .unwrap()
        .manual_experiments(None)
        .unwrap()
        .is_empty());

    // A draft may not move to another environment than its plan's.
    let record = writer
        .create_manual_draft(&failover_record(Some(&plan)))
        .unwrap();
    let err = writer
        .update_manual_draft(&record, &staging, "carol")
        .unwrap_err();
    assert_unusable(&err, "plans target_environment 'production'");

    // A plan field that is empty matches anything.
    let open = NewExercisePlan {
        target_system: None,
        target_environment: None,
        function_id: None,
        ..failover_plan()
    };
    let open = writer.create_manual_plan(&open, "alice").unwrap();
    let mut anywhere = NewManualExperiment {
        target_system: Some("ledger-db".into()),
        target_environment: Some("staging".into()),
        ..failover_record(Some(&open))
    };
    anywhere.exercise.links.clear();
    writer.create_manual_draft(&anywhere).unwrap();
}

#[test]
fn a_plan_moved_to_another_environment_after_the_draft_refuses_the_verify() {
    let (_d, store, writer) = temp_writer();
    let plan = writer
        .create_manual_plan(&failover_plan(), "alice")
        .unwrap();
    let record = writer
        .create_manual_draft(&failover_record(Some(&plan)))
        .unwrap();
    writer.submit_manual(&record, None, "carol").unwrap();
    let moved = PlanEdit {
        target_environment: Some(Some("dr-site".into())),
        ..PlanEdit::default()
    };
    writer.edit_manual_plan(&plan, &moved, "alice").unwrap();

    let err = writer.verify_manual(&record, "bob", None).unwrap_err();
    assert_unusable(
        &err,
        "plans target_environment 'dr-site', the record has 'production'",
    );
    let detail = store
        .read_only()
        .unwrap()
        .manual_experiment_detail(&record)
        .unwrap()
        .unwrap();
    assert_eq!(detail.experiment["status"], "submitted");
    let kept = plan_of(&store, &plan);
    assert_eq!(kept.plan["status"], "planned");
    assert_eq!(actions(&kept), ["create", "edit"]);
}

#[test]
fn a_bad_plan_is_refused_before_anything_is_written() {
    let (_d, store, writer) = temp_writer();
    let cases: [(NewExercisePlan, &str); 6] = [
        (
            NewExercisePlan {
                owner: "  ".into(),
                ..failover_plan()
            },
            "'owner' must not be empty",
        ),
        (
            NewExercisePlan {
                name: String::new(),
                ..failover_plan()
            },
            "'name' must not be empty",
        ),
        (
            NewExercisePlan {
                interval_days: Some(0),
                ..failover_plan()
            },
            "'interval_days' must be from 1 to 3650",
        ),
        (
            NewExercisePlan {
                interval_days: Some(3651),
                ..failover_plan()
            },
            "'interval_days' must be from 1 to 3650",
        ),
        (
            NewExercisePlan {
                planned_at_ns: Some(101 * DAY),
                ..failover_plan()
            },
            "'planned_at_ns' must not be after 'due_at_ns'",
        ),
        (
            NewExercisePlan {
                rto_target_s: Some(-1.0),
                ..failover_plan()
            },
            "'rto_target_s' must be a number of seconds at or above 0",
        ),
    ];
    for (plan, needle) in cases {
        let err = writer.create_manual_plan(&plan, "alice").unwrap_err();
        assert!(
            matches!(&err, ManualError::Plan(PlanError::Invalid(msg)) if msg.contains(needle)),
            "{err:?} should say {needle}"
        );
    }
    let err = writer
        .create_manual_plan(&failover_plan(), " ")
        .unwrap_err();
    assert!(
        matches!(&err, ManualError::Plan(PlanError::Invalid(_))),
        "{err:?}"
    );

    // An edit is checked the same way, and an edit that names no field is
    // no edit.
    let id = writer
        .create_manual_plan(&failover_plan(), "alice")
        .unwrap();
    let late = PlanEdit {
        due_at_ns: Some(80 * DAY),
        ..PlanEdit::default()
    };
    let err = writer.edit_manual_plan(&id, &late, "bob").unwrap_err();
    assert!(
        matches!(&err, ManualError::Plan(PlanError::Invalid(msg)) if msg.contains("'planned_at_ns' must not be after")),
        "{err:?}"
    );
    let err = writer
        .edit_manual_plan(&id, &PlanEdit::default(), "bob")
        .unwrap_err();
    assert!(
        matches!(&err, ManualError::Plan(PlanError::Invalid(msg)) if msg.contains("names no field")),
        "{err:?}"
    );
    let err = writer
        .edit_manual_plan("no-such-plan", &late, "bob")
        .unwrap_err();
    assert!(
        matches!(&err, ManualError::Plan(PlanError::NotFound(id)) if id == "no-such-plan"),
        "{err:?}"
    );

    assert_eq!(all_plans(&store).len(), 1);
    assert_eq!(plan_of(&store, &id).audit.len(), 1);
    let reader = store.read_only().unwrap();
    let audit = reader
        .query_json_rows("SELECT count(*) AS n FROM manual_plan_audit")
        .unwrap();
    assert_eq!(audit[0]["n"], 1);
}

fn ids(rows: &[Value]) -> BTreeSet<String> {
    rows.iter()
        .map(|r| r["id"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_plan_list_filters_by_status_target_and_due_date() {
    let (_d, store, writer) = temp_writer();
    let failover = writer
        .create_manual_plan(&failover_plan(), "alice")
        .unwrap();
    let restore = NewExercisePlan {
        name: "ledger restore".into(),
        exercise_type: ExerciseType::Restore,
        target_system: Some("ledger".into()),
        planned_at_ns: None,
        due_at_ns: 200 * DAY,
        ..failover_plan()
    };
    let restore = writer.create_manual_plan(&restore, "alice").unwrap();
    let dropped = writer
        .create_manual_plan(&failover_plan(), "alice")
        .unwrap();
    writer.cancel_manual_plan(&dropped, "alice", None).unwrap();

    let reader = store.read_only().unwrap();
    let list = |filter: PlanFilter| ids(&reader.manual_plans(&filter).unwrap());
    let set = |items: &[&String]| items.iter().map(|s| (*s).clone()).collect::<BTreeSet<_>>();

    assert_eq!(
        list(PlanFilter::default()),
        set(&[&failover, &restore, &dropped])
    );
    assert_eq!(
        list(PlanFilter {
            status: Some("planned".into()),
            ..PlanFilter::default()
        }),
        set(&[&failover, &restore])
    );
    assert_eq!(
        list(PlanFilter {
            target_system: Some("ledger".into()),
            ..PlanFilter::default()
        }),
        set(&[&restore])
    );
    assert_eq!(
        list(PlanFilter {
            status: Some("planned".into()),
            due_before_ns: Some(150 * DAY),
            ..PlanFilter::default()
        }),
        set(&[&failover])
    );
    // A value is bound, never read as SQL.
    assert!(list(PlanFilter {
        target_system: Some("x' OR '1'='1".into()),
        ..PlanFilter::default()
    })
    .is_empty());

    // Soonest due first.
    let rows = reader.manual_plans(&PlanFilter::default()).unwrap();
    let due: Vec<i64> = rows
        .iter()
        .map(|r| r["due_at_ns"].as_i64().unwrap())
        .collect();
    let mut sorted = due.clone();
    sorted.sort_unstable();
    assert_eq!(due, sorted);
}

/// Every row of `table`, as JSON, in a stable order.
fn rows(reader: &Reader, table: &str) -> Vec<String> {
    let mut rows: Vec<String> = reader
        .query_json_rows(&format!("SELECT * FROM {table}"))
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect();
    rows.sort();
    rows
}

#[test]
fn the_lake_export_carries_the_plans_and_their_audit() {
    let (dir, store, writer) = temp_writer();
    let id = writer
        .create_manual_plan(&failover_plan(), "alice")
        .unwrap();
    let edit = PlanEdit {
        due_at_ns: Some(110 * DAY),
        ..PlanEdit::default()
    };
    writer.edit_manual_plan(&id, &edit, "bob").unwrap();
    writer.cancel_manual_plan(&id, "carol", None).unwrap();
    let reader = store.read_only().unwrap();
    let cfg = LakeConfig::new(dir.path().join("lake"), 0);
    export(&reader, &cfg).unwrap();

    let other_dir = tempfile::TempDir::new().unwrap();
    let other = Store::open(&other_dir.path().join("kronika.duckdb")).unwrap();
    let restore = other.writer().unwrap();
    for table in ["manual_exercise_plans", "manual_plan_audit"] {
        let file = committed_file(&cfg, table).unwrap();
        restore
            .execute(
                &format!(
                    "INSERT INTO {table} BY NAME SELECT * FROM read_parquet('{}')",
                    file.display()
                ),
                [],
            )
            .unwrap();
    }
    let restored = other.read_only().unwrap();
    for table in ["manual_exercise_plans", "manual_plan_audit"] {
        let before = rows(&reader, table);
        assert!(!before.is_empty(), "{table} has rows to carry");
        assert_eq!(rows(&restored, table), before, "{table}");
    }
    assert_eq!(rows(&restored, "manual_plan_audit").len(), 3);
}
