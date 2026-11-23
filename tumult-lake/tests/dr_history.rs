//! Schema v25: the DR history of the lake and its due register.
//!
//! The view `dr_tests` holds one row per DR test: every run of kind `dr`,
//! and every submitted or verified manual record of a type that counts as
//! DR. The due register says, per target, when its last DR test ran, when
//! the next one is due and whether it is `ok`, `due`, `overdue` or
//! `never_tested`.

#![cfg(feature = "duckdb")]

use serde_json::{json, Value};
use tumult_core::types::{
    Experiment, ExperimentStatus, ExperimentTarget, Journal, NfrCriticality, NfrKind, NfrOperator,
    NfrState, NfrVerdict, ObjectiveOutcome, PostResult, TestKind,
};
use tumult_lake::duckdb_store::sample_journal;
use tumult_lake::{
    DrScope, ExerciseDetail, ExerciseType, IctFunction, IctFunctionService, IctLegalEntity,
    IctRegister, ManualOutcome, NewExercisePlan, NewManualExperiment, PlannedTest, Store, Writer,
    CURRENT_SCHEMA_VERSION, DEFAULT_DR_EXERCISE_TYPES,
};

const S: i64 = 1_000_000_000;
const DAY: i64 = 86_400 * S;
/// The time the register is read at.
const AS_OF: i64 = 1_800_000_000 * S;

fn temp_store() -> (tempfile::TempDir, Store) {
    let d = tempfile::TempDir::new().unwrap();
    let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
    (d, store)
}

/// What a DR run saw and how its RTO of 10 s came out.
#[derive(Clone, Copy)]
enum Seen {
    /// Out, and back 4 s later.
    Met,
    /// Out, and back 14 s later.
    Late,
    /// Out, and never back while the post phase watched.
    NeverBack,
    /// No probe saw an outage.
    NoOutage,
}

/// The post phase of a run that saw `seen`, its method ending at `end_ns`.
fn post(seen: Seen, end_ns: i64) -> PostResult {
    let (outage, back): (Option<i64>, Option<i64>) = match seen {
        Seen::Met => (Some(1), Some(3)),
        Seen::Late => (Some(1), Some(13)),
        Seen::NeverBack => (Some(1), None),
        Seen::NoOutage => (None, None),
    };
    let outage_started_at_ns = outage.map(|s| end_ns - s * S);
    let recovered_at_ns = back.map(|s| end_ns + s * S);
    #[allow(clippy::cast_precision_loss)]
    let mttr_s = outage_started_at_ns
        .zip(recovered_at_ns)
        .map(|(from, to)| (to - from) as f64 / 1e9);
    PostResult {
        started_at_ns: end_ns,
        ended_at_ns: end_ns + 30 * S,
        duration_s: 30.0,
        samples: 30,
        probes: vec![],
        recovery_time_s: 29.1,
        full_recovery: back.is_some() || outage.is_none(),
        mttr_s,
        impact_observed: Some(outage.is_some()),
        outage_started_at_ns,
        recovered_at_ns,
    }
}

/// The verdict of the run on its RTO of 10 s.
fn rto_verdict(seen: Seen, post: &PostResult) -> NfrVerdict {
    let (observed, state, recovered, reason) = match seen {
        Seen::Met => (post.mttr_s, NfrState::Met, Some(true), None),
        Seen::Late => (post.mttr_s, NfrState::Breached, Some(true), None),
        Seen::NeverBack => (Some(30.1), NfrState::Breached, Some(false), None),
        Seen::NoOutage => (
            None,
            NfrState::Inconclusive,
            None,
            Some(NfrVerdict::NO_OUTAGE_OBSERVED.to_string()),
        ),
    };
    NfrVerdict {
        nfr_id: "rto".into(),
        kind: NfrKind::Rto,
        metric: "recovery.mttr_s".into(),
        operator: NfrOperator::Lte,
        threshold: 10.0,
        observed,
        state,
        margin: None,
        criticality: NfrCriticality::Blocking,
        recovered,
        reason,
    }
}

/// A run of `kind` against `target` in `prod` that started `days_ago`
/// days before [`AS_OF`], ended with `status` and saw `seen`, with an
/// RTO of 10 s. `target` `None` declares no target.
fn run(
    id: &str,
    kind: TestKind,
    target: Option<&str>,
    days_ago: i64,
    status: ExperimentStatus,
    seen: Seen,
) -> (Journal, Experiment) {
    let mut journal = sample_journal(id, status);
    journal.kind = kind;
    journal.experiment_title = format!("{id} run");
    journal.started_at_ns = AS_OF - days_ago * DAY;
    journal.ended_at_ns = journal.started_at_ns + 60 * S;
    let post = post(seen, journal.started_at_ns + 20 * S);
    journal.objectives = Some(ObjectiveOutcome {
        rto_target_s: Some(10.0),
        rto_measured_s: post.mttr_s,
        verdicts: vec![rto_verdict(seen, &post)],
        ..ObjectiveOutcome::default()
    });
    journal.post_result = Some(post);
    let definition = Experiment {
        title: journal.experiment_title.clone(),
        target: target.map(|system| ExperimentTarget {
            system: system.into(),
            technology: None,
            environment: Some("prod".into()),
            criticality: None,
            namespace: None,
            version: None,
        }),
        ..Experiment::default()
    };
    (journal, definition)
}

/// A DR run against `target` that started `days_ago` days before
/// [`AS_OF`] and recovered within its RTO.
fn dr_run(id: &str, target: &str, days_ago: i64) -> (Journal, Experiment) {
    run(
        id,
        TestKind::Dr,
        Some(target),
        days_ago,
        ExperimentStatus::Completed,
        Seen::Met,
    )
}

fn ingest(writer: &Writer, runs: &[(Journal, Experiment)]) {
    for (journal, definition) in runs {
        assert!(writer.ingest_journal(journal, Some(definition)).unwrap());
    }
}

/// A manual exercise of `exercise` against `target` in `prod`, run
/// `days_ago` days before [`AS_OF`].
fn exercise(
    name: &str,
    exercise: ExerciseType,
    target: &str,
    days_ago: i64,
) -> NewManualExperiment {
    NewManualExperiment {
        experiment_name: name.into(),
        exercise_type: exercise,
        executed_at_ns: AS_OF - days_ago * DAY,
        hypothesis: "The standby takes the writes within its RTO".into(),
        method: "Stop the primary and promote the standby".into(),
        outcome: ManualOutcome::Passed,
        hypothesis_met: Some(true),
        findings: None,
        action_items: vec![],
        target_system: Some(target.into()),
        target_environment: Some("prod".into()),
        blast_radius: None,
        recovery_time_s: None,
        duration_s: None,
        entered_by: "carol".into(),
        attestation: "I attest this record reflects the exercise as executed.".into(),
        renewal_due_ns: None,
        framework_refs: vec![],
        exercise: ExerciseDetail::default(),
    }
}

/// Where a manual record ends up.
#[derive(Clone, Copy)]
enum Review {
    Draft,
    Submitted,
    Verified,
    Rejected,
}

fn record(writer: &Writer, new: &NewManualExperiment, review: Review) -> String {
    let id = writer.create_manual_draft(new).unwrap();
    if matches!(review, Review::Draft) {
        return id;
    }
    writer.submit_manual(&id, None, "carol").unwrap();
    match review {
        Review::Verified => writer.verify_manual(&id, "bob", None).unwrap(),
        Review::Rejected => writer.reject_manual(&id, "bob", "no logs").unwrap(),
        Review::Draft | Review::Submitted => {}
    }
    id
}

/// A plan of `exercise` against `target` in `prod`, due `due_in_days`
/// days after [`AS_OF`].
fn plan(
    name: &str,
    exercise: ExerciseType,
    target: &str,
    due_in_days: i64,
    interval_days: Option<i32>,
) -> NewExercisePlan {
    NewExercisePlan {
        name: name.into(),
        exercise_type: exercise,
        target_system: Some(target.into()),
        target_environment: Some("prod".into()),
        function_id: None,
        scope: None,
        owner: "alice".into(),
        planned_at_ns: None,
        due_at_ns: AS_OF + due_in_days * DAY,
        interval_days,
        rto_target_s: None,
        rpo_target_s: None,
    }
}

/// The register of one function, `cif-0001`, whose service glob `bill*`
/// takes in the target `billing` and whose test plan asks for a DR test
/// every 180 days.
fn register() -> IctRegister {
    IctRegister {
        legal_entities: vec![IctLegalEntity {
            lei: "529900T8BM49AURSDO55".into(),
            name: "Demo Firm".into(),
            profile: "full".into(),
            ..IctLegalEntity::default()
        }],
        functions: vec![IctFunction {
            function_id: "cif-0001".into(),
            lei: "529900T8BM49AURSDO55".into(),
            name: "Billing".into(),
            licensed_activity: "payment services".into(),
            tier: "critical".into(),
            test_plan: vec![
                PlannedTest {
                    kind: "chaos".into(),
                    interval_days: 30,
                },
                PlannedTest {
                    kind: "dr".into(),
                    interval_days: 180,
                },
            ],
            active: true,
            ..IctFunction::default()
        }],
        services: vec![IctFunctionService {
            function_id: "cif-0001".into(),
            kind: "service".into(),
            pattern: "bill*".into(),
        }],
    }
}

fn default_scope() -> DrScope {
    DrScope {
        dr_types: DEFAULT_DR_EXERCISE_TYPES
            .iter()
            .map(|t| t.as_str().to_string())
            .collect(),
        envs: vec![],
    }
}

/// The rows of the view, keyed by evidence id.
fn view_rows(store: &Store) -> Vec<Value> {
    store
        .read_only()
        .unwrap()
        .query_json_rows("SELECT * FROM dr_tests ORDER BY evidence_id")
        .unwrap()
}

fn row_of<'a>(rows: &'a [Value], evidence_id: &str) -> &'a Value {
    rows.iter()
        .find(|r| r["evidence_id"] == evidence_id)
        .unwrap_or_else(|| panic!("no row of {evidence_id} in {rows:?}"))
}

fn evidence_ids(rows: &[Value]) -> Vec<&str> {
    let mut ids: Vec<&str> = rows
        .iter()
        .map(|r| r["evidence_id"].as_str().unwrap())
        .collect();
    ids.sort_unstable();
    ids
}

#[test]
fn the_schema_is_at_version_25() {
    assert_eq!(CURRENT_SCHEMA_VERSION, 25);
}

#[test]
fn the_view_holds_each_dr_run_with_the_verdict_its_numbers_give() {
    let (_d, store) = temp_store();
    let writer = store.writer().unwrap();
    let completed = || ExperimentStatus::Completed;
    let deviated = || ExperimentStatus::Deviated;
    ingest(
        &writer,
        &[
            run(
                "met",
                TestKind::Dr,
                Some("ledger"),
                3,
                completed(),
                Seen::Met,
            ),
            run(
                "late",
                TestKind::Dr,
                Some("ledger"),
                2,
                deviated(),
                Seen::Late,
            ),
            run(
                "never-back",
                TestKind::Dr,
                Some("ledger"),
                1,
                deviated(),
                Seen::NeverBack,
            ),
            run(
                "no-outage",
                TestKind::Dr,
                Some("ledger"),
                1,
                completed(),
                Seen::NoOutage,
            ),
            run(
                "chaos",
                TestKind::Chaos,
                Some("ledger"),
                1,
                completed(),
                Seen::Met,
            ),
        ],
    );

    let rows = view_rows(&store);

    assert_eq!(
        evidence_ids(&rows),
        ["late", "met", "never-back", "no-outage"],
        "a chaos run is no DR test"
    );
    assert_eq!(
        row_of(&rows, "met"),
        &json!({
            "test_id": "automated:met",
            "origin": "automated",
            "experiment_name": "met run",
            "exercise_type": null,
            "target_system": "ledger",
            "target_environment": "prod",
            "executed_at_ns": AS_OF - 3 * DAY,
            "outcome": "completed",
            "rto_target_s": 10.0,
            "rto_measured_s": 4.0,
            "recovered": true,
            "no_outage": false,
            "rpo_target_s": null,
            "rpo_measured_s": null,
            "data_verified": null,
            "verdict": "met",
            "evidence_id": "met",
            "reviewer": null,
            "plan_id": null,
        })
    );
    let verdict = |id: &str| {
        let row = row_of(&rows, id);
        (
            row["verdict"].as_str().unwrap().to_string(),
            row["rto_measured_s"].clone(),
            row["recovered"].clone(),
        )
    };
    assert_eq!(
        verdict("late"),
        ("breached".into(), json!(14.0), json!(true))
    );
    // A run that never recovered measured no RTO: the view does not
    // turn the time it watched into one.
    assert_eq!(
        verdict("never-back"),
        ("not_recovered".into(), Value::Null, json!(false))
    );
    assert_eq!(
        verdict("no-outage"),
        ("not_measured".into(), Value::Null, Value::Null)
    );
    assert_eq!(
        row_of(&rows, "no-outage")["no_outage"],
        true,
        "the row says why it measured nothing"
    );
    assert_eq!(row_of(&rows, "never-back")["no_outage"], false);
    assert_eq!(row_of(&rows, "late")["outcome"], "deviated");
}

#[test]
fn the_view_holds_a_manual_dr_record_once_it_is_submitted_with_its_facts() {
    let (_d, store) = temp_store();
    let writer = store.writer().unwrap();
    let plan_id = writer
        .create_manual_plan(
            &plan(
                "ledger failover",
                ExerciseType::Failover,
                "ledger",
                10,
                Some(365),
            ),
            "alice",
        )
        .unwrap();
    let mut failover = exercise("ledger failover", ExerciseType::Failover, "ledger", 5);
    failover.recovery_time_s = Some(600.0);
    failover.exercise = ExerciseDetail {
        rto_target_s: Some(900.0),
        rpo_target_s: Some(60.0),
        rpo_measured_s: Some(90.0),
        data_verified: Some(false),
        plan_id: Some(plan_id.clone()),
        ..ExerciseDetail::default()
    };
    let verified = record(&writer, &failover, Review::Verified);
    let mut restore = exercise("ledger restore", ExerciseType::Restore, "ledger", 4);
    restore.recovery_time_s = Some(300.0);
    restore.exercise.rto_target_s = Some(900.0);
    let submitted = record(&writer, &restore, Review::Submitted);
    let draft = record(
        &writer,
        &exercise("ledger drill", ExerciseType::Drill, "ledger", 3),
        Review::Draft,
    );
    let rejected = record(
        &writer,
        &exercise("ledger backup", ExerciseType::Backup, "ledger", 2),
        Review::Rejected,
    );
    let tabletop = record(
        &writer,
        &exercise("ledger tabletop", ExerciseType::Tabletop, "ledger", 1),
        Review::Verified,
    );

    let rows = view_rows(&store);

    let mut expected = vec![verified.as_str(), submitted.as_str()];
    expected.sort_unstable();
    assert_eq!(
        evidence_ids(&rows),
        expected,
        "a draft {draft}, a rejected record {rejected} and a tabletop {tabletop} are left out"
    );
    assert_eq!(
        row_of(&rows, &verified),
        &json!({
            "test_id": format!("manual:{verified}"),
            "origin": "manual",
            "experiment_name": "ledger failover",
            "exercise_type": "failover",
            "target_system": "ledger",
            "target_environment": "prod",
            "executed_at_ns": AS_OF - 5 * DAY,
            "outcome": "passed",
            "rto_target_s": 900.0,
            "rto_measured_s": 600.0,
            "recovered": true,
            "no_outage": null,
            "rpo_target_s": 60.0,
            "rpo_measured_s": 90.0,
            "data_verified": false,
            "verdict": "breached",
            "evidence_id": verified,
            "reviewer": "bob",
            "plan_id": plan_id,
        })
    );
    let restore_row = row_of(&rows, &submitted);
    assert_eq!(restore_row["verdict"], "met", "{restore_row}");
    assert_eq!(restore_row["reviewer"], Value::Null, "{restore_row}");
}

#[test]
fn a_reader_counts_the_manual_types_the_house_maps_to_dr_and_keeps_to_its_environments() {
    let (_d, store) = temp_store();
    let writer = store.writer().unwrap();
    let tabletop = record(
        &writer,
        &exercise("ledger tabletop", ExerciseType::Tabletop, "ledger", 1),
        Review::Verified,
    );
    let failover = record(
        &writer,
        &exercise("ledger failover", ExerciseType::Failover, "ledger", 1),
        Review::Verified,
    );
    let mut staging = run(
        "staging-run",
        TestKind::Dr,
        Some("ledger"),
        1,
        ExperimentStatus::Completed,
        Seen::Met,
    );
    if let Some(target) = staging.1.target.as_mut() {
        target.environment = Some("staging".into());
    }
    ingest(&writer, &[staging, dr_run("prod-run", "ledger", 1)]);
    let reader = store.read_only().unwrap();
    let ids = |scope: &DrScope| {
        let rows = reader.dr_tests(scope).unwrap();
        evidence_ids(&rows)
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };

    let house = DrScope {
        dr_types: vec!["tabletop".into()],
        envs: vec![],
    };
    let mut expected = vec!["prod-run".to_string(), "staging-run".into(), tabletop];
    expected.sort_unstable();
    assert_eq!(ids(&house), expected);
    let mut expected = vec!["prod-run".to_string(), "staging-run".into(), failover];
    expected.sort_unstable();
    assert_eq!(ids(&default_scope()), expected);
    let staging_only = DrScope {
        envs: vec!["staging".into()],
        ..default_scope()
    };
    assert_eq!(ids(&staging_only), ["staging-run"]);
}

/// The register of `store` at [`AS_OF`], keyed by target.
fn register_of(store: &Store, scope: &DrScope) -> Vec<Value> {
    store
        .read_only()
        .unwrap()
        .dr_due_register(scope, AS_OF)
        .unwrap()
}

fn due_of<'a>(rows: &'a [Value], target: &str) -> &'a Value {
    rows.iter()
        .find(|r| r["target_system"] == target)
        .unwrap_or_else(|| panic!("no register row of {target} in {rows:?}"))
}

/// The ids of what [`seed_register`] stored that the register names.
struct Seeded {
    catalog: String,
    ledger: String,
    payments_plan: String,
    archive_plan: String,
}

/// A lake for the register at [`AS_OF`]. `orders` was DR tested 10 days
/// ago, and an aborted run since. `billing` 200 days ago, bound to a
/// function that asks for a DR test every 180 days. `payments-db` 100 days
/// ago, with a planned failover every 90 days. `catalog` had a restore 350
/// days ago and `ledger` a failover 400 days ago. `search` declares an RTO
/// on a chaos run, and `archive` has a planned backup, both never DR
/// tested. `drafts` has only a draft and a rejected record, `audit` only a
/// planned tabletop, and one DR run names no target.
fn seed_register(writer: &Writer) -> Seeded {
    writer.sync_ict_register(&register(), AS_OF).unwrap();
    ingest(
        writer,
        &[
            dr_run("orders-10", "orders", 10),
            // An aborted run proves nothing: it is no last test.
            run(
                "orders-aborted",
                TestKind::Dr,
                Some("orders"),
                1,
                ExperimentStatus::Aborted,
                Seen::Met,
            ),
            dr_run("billing-200", "billing", 200),
            dr_run("payments-100", "payments-db", 100),
            // A chaos run that declares an RTO: its target needs a DR test.
            run(
                "search-chaos",
                TestKind::Chaos,
                Some("search"),
                1,
                ExperimentStatus::Completed,
                Seen::Met,
            ),
            run(
                "nowhere",
                TestKind::Dr,
                None,
                1,
                ExperimentStatus::Completed,
                Seen::Met,
            ),
        ],
    );
    let catalog = record(
        writer,
        &exercise("catalog restore", ExerciseType::Restore, "catalog", 350),
        Review::Verified,
    );
    let ledger = record(
        writer,
        &exercise("ledger failover", ExerciseType::Failover, "ledger", 400),
        Review::Submitted,
    );
    let mut draft = exercise("drafts drill", ExerciseType::Drill, "drafts", 1);
    draft.exercise.rto_target_s = Some(60.0);
    record(writer, &draft, Review::Draft);
    record(
        writer,
        &exercise("drafts failover", ExerciseType::Failover, "drafts", 1),
        Review::Rejected,
    );
    let payments_plan = writer
        .create_manual_plan(
            &plan(
                "payments-db failover",
                ExerciseType::Failover,
                "payments-db",
                5,
                Some(90),
            ),
            "alice",
        )
        .unwrap();
    let archive_plan = writer
        .create_manual_plan(
            &plan("archive backup", ExerciseType::Backup, "archive", 20, None),
            "alice",
        )
        .unwrap();
    writer
        .create_manual_plan(
            &plan(
                "audit tabletop",
                ExerciseType::Tabletop,
                "audit",
                20,
                Some(30),
            ),
            "alice",
        )
        .unwrap();
    Seeded {
        catalog,
        ledger,
        payments_plan,
        archive_plan,
    }
}

#[test]
fn the_register_says_when_the_next_dr_test_of_each_target_is_due() {
    let (_d, store) = temp_store();
    let Seeded {
        catalog,
        ledger,
        payments_plan,
        archive_plan,
    } = seed_register(&store.writer().unwrap());

    let rows = register_of(&store, &default_scope());

    let targets: Vec<&str> = rows
        .iter()
        .map(|r| r["target_system"].as_str().unwrap())
        .collect();
    assert_eq!(
        targets,
        [
            "archive",
            "billing",
            "catalog",
            "ledger",
            "orders",
            "payments-db",
            "search"
        ],
        "a draft, a rejected record, a plan that is no DR and a run with no target add no target"
    );
    assert_eq!(
        due_of(&rows, "orders"),
        &json!({
            "target_system": "orders",
            "target_environment": "prod",
            "last_test_at_ns": AS_OF - 10 * DAY,
            "last_test_id": "automated:orders-10",
            "interval_days": 365,
            "interval_source": "default",
            "next_due_at_ns": AS_OF + 355 * DAY,
            "state": "ok",
            "plan_id": null,
            "plan_due_at_ns": null,
        })
    );
    let state = |target: &str| {
        let row = due_of(&rows, target);
        (
            row["state"].as_str().unwrap().to_string(),
            row["interval_days"].as_i64().unwrap(),
            row["interval_source"].as_str().unwrap().to_string(),
            row["next_due_at_ns"].as_i64(),
        )
    };
    assert_eq!(
        state("catalog"),
        ("due".into(), 365, "default".into(), Some(AS_OF + 15 * DAY))
    );
    assert_eq!(
        due_of(&rows, "catalog")["last_test_id"],
        format!("manual:{catalog}")
    );
    assert_eq!(
        state("ledger"),
        (
            "overdue".into(),
            365,
            "default".into(),
            Some(AS_OF - 35 * DAY)
        )
    );
    assert_eq!(
        due_of(&rows, "ledger")["last_test_id"],
        format!("manual:{ledger}"),
        "a submitted record is a DR test"
    );
    assert_eq!(
        state("billing"),
        (
            "overdue".into(),
            180,
            "function".into(),
            Some(AS_OF - 20 * DAY)
        )
    );
    assert_eq!(
        state("payments-db"),
        ("overdue".into(), 90, "plan".into(), Some(AS_OF - 10 * DAY))
    );
    assert_eq!(due_of(&rows, "payments-db")["plan_id"], payments_plan);
    assert_eq!(
        due_of(&rows, "payments-db")["plan_due_at_ns"],
        AS_OF + 5 * DAY
    );
    assert_eq!(
        state("search"),
        ("never_tested".into(), 365, "default".into(), None)
    );
    assert_eq!(
        state("archive"),
        ("never_tested".into(), 365, "default".into(), None)
    );
    assert_eq!(due_of(&rows, "archive")["plan_id"], archive_plan);
    assert_eq!(due_of(&rows, "archive")["target_environment"], "prod");
}

#[test]
fn a_run_that_saw_no_outage_is_no_last_dr_test_of_its_target() {
    let (_d, store) = temp_store();
    let writer = store.writer().unwrap();
    let no_outage = |id: &str, target: &str, days_ago: i64| {
        run(
            id,
            TestKind::Dr,
            Some(target),
            days_ago,
            ExperimentStatus::Completed,
            Seen::NoOutage,
        )
    };
    ingest(
        &writer,
        &[
            dr_run("vault-340", "vault", 340),
            no_outage("vault-5", "vault", 5),
            no_outage("cache-3", "cache", 3),
            no_outage("cache-2", "cache", 2),
        ],
    );
    // A manual record counts as it did, with or without a recovery time.
    let files = record(
        &writer,
        &exercise("files restore", ExerciseType::Restore, "files", 10),
        Review::Verified,
    );

    let rows = register_of(&store, &default_scope());

    let vault = due_of(&rows, "vault");
    assert_eq!(vault["state"], "due", "{vault}");
    assert_eq!(vault["last_test_id"], "automated:vault-340");
    assert_eq!(vault["next_due_at_ns"], AS_OF + 25 * DAY);
    let cache = due_of(&rows, "cache");
    assert_eq!(cache["state"], "never_tested", "{cache}");
    assert_eq!(cache["last_test_at_ns"], Value::Null);
    let files_row = due_of(&rows, "files");
    assert_eq!(files_row["state"], "ok", "{files_row}");
    assert_eq!(files_row["last_test_id"], format!("manual:{files}"));
    // The view keeps every one of those runs.
    assert_eq!(
        evidence_ids(&view_rows(&store))
            .into_iter()
            .filter(|id| id.starts_with("vault") || id.starts_with("cache"))
            .collect::<Vec<_>>(),
        ["cache-2", "cache-3", "vault-340", "vault-5"]
    );
}

#[test]
fn the_register_keeps_to_the_environments_it_is_asked_for() {
    let (_d, store) = temp_store();
    let writer = store.writer().unwrap();
    let mut staging = dr_run("staging-run", "orders", 1);
    if let Some(target) = staging.1.target.as_mut() {
        target.environment = Some("staging".into());
    }
    ingest(&writer, &[staging, dr_run("prod-run", "orders", 400)]);

    let scope = |envs: &[&str]| DrScope {
        envs: envs.iter().map(|e| (*e).to_string()).collect(),
        ..default_scope()
    };
    let states = |envs: &[&str]| -> Vec<(String, String)> {
        register_of(&store, &scope(envs))
            .iter()
            .map(|r| {
                (
                    r["target_environment"].as_str().unwrap().to_string(),
                    r["state"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    };

    assert_eq!(
        states(&[]),
        [
            ("prod".to_string(), "overdue".to_string()),
            ("staging".into(), "ok".into())
        ]
    );
    assert_eq!(
        states(&["staging"]),
        [("staging".to_string(), "ok".to_string())]
    );
}

/// A lake as v24 left it: the same tables, no `dr_tests` view, and the
/// version set back to 24.
fn a_v24_lake(path: &std::path::Path) {
    let store = Store::open(path).unwrap();
    let writer = store.writer().unwrap();
    ingest(&writer, &[dr_run("old-run", "ledger", 30)]);
    let mut failover = exercise("ledger failover", ExerciseType::Failover, "ledger", 20);
    failover.recovery_time_s = Some(600.0);
    record(&writer, &failover, Review::Verified);
    for sql in [
        "DROP VIEW dr_tests",
        "UPDATE schema_meta SET value = 24 WHERE key = 'version'",
    ] {
        writer.execute(sql, []).unwrap();
    }
    assert_eq!(writer.schema_version().unwrap(), 24);
}

#[test]
fn a_v24_lake_gains_the_dr_history_and_keeps_its_runs_and_records() {
    let d = tempfile::TempDir::new().unwrap();
    let path = d.path().join("kronika.duckdb");
    a_v24_lake(&path);
    {
        let reader = Store::at(&path).read_only().unwrap();
        assert!(
            reader.query_json_rows("SELECT * FROM dr_tests").is_err(),
            "the v24 lake has no view"
        );
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

    let rows = view_rows(&store);
    let origins: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| {
            (
                r["origin"].as_str().unwrap(),
                r["verdict"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(origins.len(), 2, "{rows:?}");
    assert!(origins.contains(&("automated", "met")), "{rows:?}");
    assert!(origins.contains(&("manual", "not_measured")), "{rows:?}");
    let register = register_of(&store, &default_scope());
    assert_eq!(register.len(), 1, "{register:?}");
    assert_eq!(register[0]["state"], "ok");
    assert_eq!(register[0]["last_test_at_ns"], AS_OF - 20 * DAY);
}
