//! `GET /api/dr`: the DR view of the environments of the caller.
//!
//! Each test seeds one lake: DR runs stored as their journals, manual DR
//! records entered and verified, a function of the register with a DR test
//! plan, and plans sent to the API. Then it reads the view through the
//! router.

use serde_json::{json, Value};
use tumult_core::types::{
    Experiment, ExperimentStatus, ExperimentTarget, Journal, NfrCriticality, NfrKind, NfrOperator,
    NfrState, NfrVerdict, ObjectiveOutcome, PostResult, TestKind,
};
use tumult_lake::{
    ExerciseDetail, ExerciseType, IctFunction, IctFunctionService, IctLegalEntity, IctRegister,
    ManualOutcome, NewManualExperiment, PlannedTest, Store,
};

use crate::common::*;

const DAY: i64 = 86_400 * NS;

/// What a DR run with an RTO of 10 s saw.
#[derive(Clone, Copy)]
enum Seen {
    /// Out, and back 4 s later: the RTO is met.
    Met,
    /// Out, and back 14 s later: the RTO is breached.
    Late,
    /// Out, and never back while the post phase watched.
    NeverBack,
    /// No probe saw an outage.
    NoOutage,
}

/// The post phase of a run that saw `seen`, its method ending at `end_ns`.
fn post_phase(seen: Seen, end_ns: i64) -> PostResult {
    let (outage, back): (Option<i64>, Option<i64>) = match seen {
        Seen::Met => (Some(1), Some(3)),
        Seen::Late => (Some(1), Some(13)),
        Seen::NeverBack => (Some(1), None),
        Seen::NoOutage => (None, None),
    };
    let outage_started_at_ns = outage.map(|s| end_ns - s * NS);
    let recovered_at_ns = back.map(|s| end_ns + s * NS);
    let mttr_s = outage_started_at_ns
        .zip(recovered_at_ns)
        .map(|(from, to)| (to - from) as f64 / 1e9);
    PostResult {
        started_at_ns: end_ns,
        ended_at_ns: end_ns + 30 * NS,
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

/// The verdict of a run on its RTO of 10 s.
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

/// A DR run `id` against `target` in `environment` that started at
/// `started_at_ns` and saw `seen`, with an RTO of 10 s.
fn dr_run(
    id: &str,
    target: &str,
    environment: &str,
    started_at_ns: i64,
    seen: Seen,
) -> (Journal, Experiment) {
    let status = match seen {
        Seen::Met | Seen::NoOutage => ExperimentStatus::Completed,
        Seen::Late | Seen::NeverBack => ExperimentStatus::Deviated,
    };
    let mut journal = tumult_lake::duckdb_store::sample_journal(id, status);
    journal.kind = TestKind::Dr;
    journal.experiment_title = format!("{target} failover");
    journal.started_at_ns = started_at_ns;
    journal.ended_at_ns = started_at_ns + 60 * NS;
    let post = post_phase(seen, started_at_ns + 20 * NS);
    journal.objectives = Some(ObjectiveOutcome {
        rto_target_s: Some(10.0),
        rto_measured_s: post.mttr_s,
        verdicts: vec![rto_verdict(seen, &post)],
        ..ObjectiveOutcome::default()
    });
    journal.post_result = Some(post);
    let definition = Experiment {
        title: journal.experiment_title.clone(),
        target: Some(ExperimentTarget {
            system: target.into(),
            technology: Some("postgresql".into()),
            environment: Some(environment.into()),
            criticality: None,
            namespace: None,
            version: None,
        }),
        ..Experiment::default()
    };
    (journal, definition)
}

/// A manual exercise of `exercise` against `target` in `prod`, run at
/// `executed_at_ns`, that came back in `recovery_s` against an RTO of
/// 900 s and lost 10 s of writes against an RPO of 60 s.
fn exercise(
    name: &str,
    exercise: ExerciseType,
    target: &str,
    executed_at_ns: i64,
    recovery_s: f64,
) -> NewManualExperiment {
    NewManualExperiment {
        experiment_name: name.into(),
        exercise_type: exercise,
        executed_at_ns,
        hypothesis: "The standby takes the writes within its RTO".into(),
        method: "Stop the primary and promote the standby".into(),
        outcome: ManualOutcome::Passed,
        hypothesis_met: Some(true),
        findings: None,
        action_items: vec![],
        target_system: Some(target.into()),
        target_environment: Some("prod".into()),
        blast_radius: None,
        recovery_time_s: Some(recovery_s),
        duration_s: None,
        entered_by: "carol".into(),
        attestation: "I attest this record reflects the exercise as executed.".into(),
        renewal_due_ns: None,
        framework_refs: vec![],
        exercise: ExerciseDetail {
            rto_target_s: Some(900.0),
            rpo_target_s: Some(60.0),
            rpo_measured_s: Some(10.0),
            data_verified: Some(true),
            ..ExerciseDetail::default()
        },
    }
}

/// One function, `cif-0001`, whose service glob `bill*` takes in the
/// target `billing` and whose test plan asks for a DR test every 180 days.
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
            test_plan: vec![PlannedTest {
                kind: "dr".into(),
                interval_days: 180,
            }],
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

async fn post(base: &str, path: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

/// What [`seed_dr`] stored.
struct Seeded {
    /// The time the seed is measured from.
    now: i64,
    catalog: String,
    ledger: String,
    payments_plan: String,
    archive_plan: String,
}

impl Seeded {
    /// `days` days before [`Seeded::now`].
    fn at(&self, days: i64) -> i64 {
        self.now - days * DAY
    }
}

/// A lake with a DR test history, every target in `prod` but `search`.
/// `orders` was DR tested 10 days ago and met its RTO. `billing` 200 days
/// ago and breached it, bound to a function that asks for a DR test every
/// 180 days. `payments-db` 100 days ago and did not recover, with a
/// planned failover every 90 days. `catalog` had a restore 350 days ago
/// and `ledger` a failover 400 days ago. `search`, in `staging`, had a run
/// 5 days ago whose probes saw no outage. `archive` has a planned backup
/// with an RTO and an RPO and was never tested.
async fn seed_dr(srv: &TestServer) -> Seeded {
    let now = now_ns();
    let at = |days: i64| now - days * DAY;
    let runs = vec![
        dr_run("orders-dr", "orders", "prod", at(10), Seen::Met),
        dr_run("billing-dr", "billing", "prod", at(200), Seen::Late),
        dr_run(
            "payments-dr",
            "payments-db",
            "prod",
            at(100),
            Seen::NeverBack,
        ),
        dr_run("search-dr", "search", "staging", at(5), Seen::NoOutage),
    ];
    let records = vec![
        exercise(
            "catalog restore",
            ExerciseType::Restore,
            "catalog",
            at(350),
            300.0,
        ),
        exercise(
            "ledger failover",
            ExerciseType::Failover,
            "ledger",
            at(400),
            600.0,
        ),
    ];
    let ids = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let stored = std::sync::Arc::clone(&ids);
    exec_write(srv, move |w| {
        w.sync_ict_register(&register(), now)
            .map_err(|e| e.to_string())?;
        for (journal, definition) in &runs {
            w.ingest_journal(journal, Some(definition))
                .map_err(|e| e.to_string())?;
        }
        for record in &records {
            let id = w.create_manual_draft(record).map_err(|e| e.to_string())?;
            w.submit_manual(&id, None, "carol")
                .map_err(|e| e.to_string())?;
            w.verify_manual(&id, "bob", None)
                .map_err(|e| e.to_string())?;
            stored.lock().unwrap().push(id);
        }
        Ok(())
    })
    .await;
    let ids = ids.lock().unwrap().clone();
    let plan = |body: Value| {
        let base = srv.base.clone();
        async move {
            let (status, made) = post(&base, "/api/manual/plans", body).await;
            assert_eq!(status, 201, "{made}");
            made["id"].as_str().unwrap().to_string()
        }
    };
    let payments_plan = plan(json!({
        "name": "payments-db failover",
        "exercise_type": "failover",
        "target_system": "payments-db",
        "target_environment": "prod",
        "owner": "alice",
        "due_at_ns": now + 5 * DAY,
        "interval_days": 90,
        "created_by": "alice",
    }))
    .await;
    let archive_plan = plan(json!({
        "name": "archive backup",
        "exercise_type": "backup",
        "target_system": "archive",
        "target_environment": "prod",
        "owner": "alice",
        "due_at_ns": now + 20 * DAY,
        "rto_target_s": 120.0,
        "rpo_target_s": 30.0,
        "created_by": "alice",
    }))
    .await;
    Seeded {
        now,
        catalog: ids[0].clone(),
        ledger: ids[1].clone(),
        payments_plan,
        archive_plan,
    }
}

/// `system/environment/state` of each target of a view.
fn states(view: &Value) -> Vec<String> {
    view["targets"]
        .as_array()
        .unwrap_or_else(|| panic!("no targets: {view}"))
        .iter()
        .map(|t| {
            format!(
                "{}/{}/{}",
                t["system"].as_str().unwrap(),
                t["environment"].as_str().unwrap_or("-"),
                t["state"].as_str().unwrap()
            )
        })
        .collect()
}

fn target_of<'a>(view: &'a Value, system: &str) -> &'a Value {
    view["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["system"] == system)
        .unwrap_or_else(|| panic!("no target {system}: {view}"))
}

/// The test ids of a view, in its order.
fn test_ids(view: &Value) -> Vec<String> {
    view["tests"]
        .as_array()
        .unwrap_or_else(|| panic!("no tests: {view}"))
        .iter()
        .map(|t| t["test_id"].as_str().unwrap().to_string())
        .collect()
}

/// A summary with these counts per state, these tests of the automated
/// runs (`met`, `breached`, `not_recovered`, `not_measured`) and of the
/// manual records.
fn summary(states: [u64; 4], automated: [u64; 4], manual: [u64; 4]) -> Value {
    let verdicts = |n: [u64; 4]| json!({"met": n[0], "breached": n[1], "not_recovered": n[2], "not_measured": n[3]});
    json!({
        "states": {
            "ok": states[0],
            "due": states[1],
            "overdue": states[2],
            "never_tested": states[3],
        },
        "tests": {
            "total": automated.iter().chain(&manual).sum::<u64>(),
            "automated": verdicts(automated),
            "manual": verdicts(manual),
        },
    })
}

#[tokio::test]
async fn each_target_has_its_state_its_last_test_and_when_the_next_one_is_due() {
    let srv = spawn_server().await;
    let seeded = seed_dr(&srv).await;

    let (status, view) = get(&srv.base, "/api/dr").await;

    assert_eq!(status, 200, "{view}");
    assert_eq!(
        states(&view),
        [
            "archive/prod/never_tested",
            "billing/prod/overdue",
            "catalog/prod/due",
            "ledger/prod/overdue",
            "orders/prod/ok",
            "payments-db/prod/overdue",
            "search/staging/never_tested",
        ],
        "a run that saw no outage is no last test"
    );
    assert_eq!(
        target_of(&view, "orders"),
        &json!({
            "system": "orders",
            "environment": "prod",
            "state": "ok",
            "last_test": {
                "id": "automated:orders-dr",
                "origin": "automated",
                "executed_at_ns": seeded.at(10),
                "verdict": "met",
            },
            "next_due_at_ns": seeded.at(10) + 365 * DAY,
            "interval_days": 365,
            "interval_source": "default",
            "plan": null,
            "rto_target_s": 10.0,
            "rpo_target_s": null,
            "rto_measured_s": 4.0,
            "rpo_measured_s": null,
            "recovered": true,
        })
    );
    assert_eq!(
        target_of(&view, "catalog"),
        &json!({
            "system": "catalog",
            "environment": "prod",
            "state": "due",
            "last_test": {
                "id": format!("manual:{}", seeded.catalog),
                "origin": "manual",
                "executed_at_ns": seeded.at(350),
                "verdict": "met",
            },
            "next_due_at_ns": seeded.at(350) + 365 * DAY,
            "interval_days": 365,
            "interval_source": "default",
            "plan": null,
            "rto_target_s": 900.0,
            "rpo_target_s": 60.0,
            "rto_measured_s": 300.0,
            "rpo_measured_s": 10.0,
            "recovered": true,
        })
    );
    let billing = target_of(&view, "billing");
    assert_eq!(billing["interval_days"], 180, "{billing}");
    assert_eq!(billing["interval_source"], "function", "{billing}");
    assert_eq!(billing["next_due_at_ns"], seeded.at(200) + 180 * DAY);
    assert_eq!(billing["last_test"]["verdict"], "breached", "{billing}");
    assert_eq!(
        (&billing["rto_target_s"], &billing["rto_measured_s"]),
        (&json!(10.0), &json!(14.0))
    );
    let ledger = target_of(&view, "ledger");
    assert_eq!(
        ledger["last_test"]["id"],
        format!("manual:{}", seeded.ledger)
    );
    assert_eq!(ledger["next_due_at_ns"], seeded.at(400) + 365 * DAY);
    assert_eq!(
        target_of(&view, "payments-db"),
        &json!({
            "system": "payments-db",
            "environment": "prod",
            "state": "overdue",
            "last_test": {
                "id": "automated:payments-dr",
                "origin": "automated",
                "executed_at_ns": seeded.at(100),
                "verdict": "not_recovered",
            },
            "next_due_at_ns": seeded.at(100) + 90 * DAY,
            "interval_days": 90,
            "interval_source": "plan",
            "plan": {"id": seeded.payments_plan, "due_at_ns": seeded.now + 5 * DAY},
            "rto_target_s": 10.0,
            "rpo_target_s": null,
            "rto_measured_s": null,
            "rpo_measured_s": null,
            "recovered": false,
        }),
        "a run that did not recover keeps no measured RTO"
    );
    assert_eq!(
        target_of(&view, "archive"),
        &json!({
            "system": "archive",
            "environment": "prod",
            "state": "never_tested",
            "last_test": null,
            "next_due_at_ns": null,
            "interval_days": 365,
            "interval_source": "plan",
            "plan": {"id": seeded.archive_plan, "due_at_ns": seeded.now + 20 * DAY},
            "rto_target_s": 120.0,
            "rpo_target_s": 30.0,
            "rto_measured_s": null,
            "rpo_measured_s": null,
            "recovered": null,
        }),
        "with no last test the targets are those of the plan"
    );
    assert_eq!(
        view["summary"]["states"],
        summary([1, 1, 3, 2], [0; 4], [0; 4])["states"]
    );
}

#[tokio::test]
async fn the_tests_of_the_last_365_days_come_newest_first_with_every_column_of_the_view() {
    let srv = spawn_server().await;
    let seeded = seed_dr(&srv).await;

    let (status, view) = get(&srv.base, "/api/dr").await;
    let after = now_ns();

    assert_eq!(status, 200, "{view}");
    let catalog = format!("manual:{}", seeded.catalog);
    assert_eq!(
        test_ids(&view),
        [
            "automated:search-dr",
            "automated:orders-dr",
            "automated:payments-dr",
            "automated:billing-dr",
            catalog.as_str(),
        ],
        "the ledger failover of 400 days ago is out of the period"
    );
    let to = view["to_ns"].as_i64().unwrap();
    assert!((seeded.now..=after).contains(&to), "{view}");
    assert_eq!(view["from_ns"], to - 365 * DAY);
    assert_eq!(view["as_of_ns"], to);
    assert_eq!(
        (
            &view["count"],
            &view["total"],
            &view["truncated"],
            &view["next_cursor"]
        ),
        (&json!(5), &json!(5), &json!(false), &Value::Null)
    );
    assert_eq!(
        view["tests"][4],
        json!({
            "test_id": catalog,
            "origin": "manual",
            "experiment_name": "catalog restore",
            "exercise_type": "restore",
            "target_system": "catalog",
            "target_environment": "prod",
            "executed_at_ns": seeded.at(350),
            "outcome": "passed",
            "rto_target_s": 900.0,
            "rto_measured_s": 300.0,
            "recovered": true,
            "no_outage": null,
            "rpo_target_s": 60.0,
            "rpo_measured_s": 10.0,
            "data_verified": true,
            "verdict": "met",
            "evidence_id": seeded.catalog,
            "reviewer": "bob",
            "plan_id": null,
        })
    );
    let search = &view["tests"][0];
    assert_eq!(
        (
            &search["no_outage"],
            &search["recovered"],
            &search["verdict"]
        ),
        (&json!(true), &Value::Null, &json!("not_measured")),
        "{search}"
    );
    let columns: Vec<String> = Store::at(&srv.db_path)
        .read_only()
        .unwrap()
        .query_json_rows(
            "SELECT column_name FROM information_schema.columns \
             WHERE table_name = 'dr_tests' ORDER BY ordinal_position",
        )
        .unwrap()
        .iter()
        .map(|c| c["column_name"].as_str().unwrap().to_string())
        .collect();
    for test in view["tests"].as_array().unwrap() {
        let keys: Vec<&String> = test.as_object().unwrap().keys().collect();
        let mut want: Vec<&String> = columns.iter().collect();
        let mut keys_sorted = keys.clone();
        keys_sorted.sort();
        want.sort();
        assert_eq!(keys_sorted, want, "every column of the view: {test}");
    }
    assert_eq!(
        view["summary"],
        summary([1, 1, 3, 2], [1, 1, 1, 1], [1, 0, 0, 0])
    );
}

#[tokio::test]
async fn a_period_keeps_the_tests_that_ran_inside_it_and_not_the_targets() {
    let srv = spawn_server().await;
    let seeded = seed_dr(&srv).await;
    let catalog = format!("manual:{}", seeded.catalog);
    let ledger = format!("manual:{}", seeded.ledger);

    let path = format!("/api/dr?from={}&to={}", seeded.at(450), seeded.at(300));
    let (status, view) = get(&srv.base, &path).await;

    assert_eq!(status, 200, "{view}");
    assert_eq!(test_ids(&view), [catalog.as_str(), ledger.as_str()]);
    assert_eq!(
        (&view["from_ns"], &view["to_ns"]),
        (&json!(seeded.at(450)), &json!(seeded.at(300)))
    );
    assert_eq!(view["total"], 2, "{view}");
    assert_eq!(
        view["summary"],
        summary([1, 1, 3, 2], [0; 4], [2, 0, 0, 0]),
        "the period selects the tests, the targets stay as they are now"
    );
    assert_eq!(states(&view).len(), 7, "{view}");

    let both_ends = format!("/api/dr?from={0}&to={0}", seeded.at(10));
    let (_, view) = get(&srv.base, &both_ends).await;
    assert_eq!(test_ids(&view), ["automated:orders-dr"], "both ends are in");

    let (_, view) = get(&srv.base, &format!("/api/dr?from={}", seeded.at(150))).await;
    assert_eq!(
        test_ids(&view),
        [
            "automated:search-dr",
            "automated:orders-dr",
            "automated:payments-dr"
        ],
        "a period with no end runs to now"
    );
    let (_, view) = get(&srv.base, &format!("/api/dr?to={}", seeded.at(150))).await;
    assert_eq!(
        test_ids(&view),
        ["automated:billing-dr", catalog.as_str(), ledger.as_str()],
        "a period with no start is the 365 days before its end"
    );
    assert_eq!(view["from_ns"], seeded.at(150) - 365 * DAY);

    let backwards = format!("/api/dr?from={}&to={}", seeded.at(1), seeded.at(2));
    let (status, body) = get(&srv.base, &backwards).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(body["error"], "from must not be later than to");
}

#[tokio::test]
async fn the_target_and_environment_filters_keep_the_targets_and_tests_they_name() {
    let srv = spawn_server().await;
    seed_dr(&srv).await;

    let (status, view) = get(&srv.base, "/api/dr?target=billing").await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(states(&view), ["billing/prod/overdue"]);
    assert_eq!(test_ids(&view), ["automated:billing-dr"]);
    assert_eq!(view["summary"], summary([0, 0, 1, 0], [0, 1, 0, 0], [0; 4]));

    let (status, view) = get(&srv.base, "/api/dr?environment=staging").await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(states(&view), ["search/staging/never_tested"]);
    assert_eq!(test_ids(&view), ["automated:search-dr"]);
    assert_eq!(view["summary"], summary([0, 0, 0, 1], [0, 0, 0, 1], [0; 4]));

    let (_, view) = get(&srv.base, "/api/dr?target=billing&environment=staging").await;
    assert_eq!(states(&view), Vec::<String>::new());
    assert_eq!(test_ids(&view), Vec::<String>::new());

    let (status, view) = get(&srv.base, "/api/dr?target=nothing-here").await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["summary"], summary([0; 4], [0; 4], [0; 4]));

    let (_, whole) = get(&srv.base, "/api/dr").await;
    let (status, view) = get(&srv.base, "/api/dr?target=&environment=").await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(
        (states(&view), test_ids(&view)),
        (states(&whole), test_ids(&whole)),
        "an empty filter keeps everything"
    );
}

#[tokio::test]
async fn a_caller_sees_the_dr_view_of_its_own_environments_only() {
    let srv = spawn_server().await;
    seed_dr(&srv).await;
    let staging = add_scoped_token(&srv, "stg-viewer", "viewer", &["staging"]).await;
    let dev = add_scoped_token(&srv, "dev-viewer", "viewer", &["dev"]).await;
    let all = add_scoped_token(&srv, "all-viewer", "viewer", &[]).await;

    let (status, view) = get_auth(&srv.base, "/api/dr", &dev).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(view["targets"], json!([]), "{view}");
    assert_eq!(view["tests"], json!([]), "{view}");
    assert_eq!(view["total"], 0, "{view}");
    assert_eq!(view["summary"], summary([0; 4], [0; 4], [0; 4]));

    let (status, view) = get_auth(&srv.base, "/api/dr", &staging).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(states(&view), ["search/staging/never_tested"]);
    assert_eq!(test_ids(&view), ["automated:search-dr"]);

    let (status, body) = get_auth(&srv.base, "/api/dr?environment=prod", &staging).await;
    assert_eq!(status, 403, "{body}");
    assert_eq!(
        body["error"],
        "environment prod is outside the principal's scopes"
    );

    let (status, view) = get_auth(&srv.base, "/api/dr", &all).await;
    assert_eq!(status, 200, "{view}");
    assert_eq!(states(&view).len(), 7, "{view}");
    assert_eq!(view["total"], 5, "{view}");

    let anonymous = reqwest::get(format!("{}/api/dr", srv.base)).await.unwrap();
    assert_eq!(anonymous.status(), 401);
}

/// Follow `next_cursor` from `path` until the tests end; the pages.
async fn pages(base: &str, path: &str) -> Vec<Value> {
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let url = match &cursor {
            Some(cursor) => format!("{path}&cursor={cursor}"),
            None => path.to_owned(),
        };
        let (status, body) = get(base, &url).await;
        assert_eq!(status, 200, "{url}: {body}");
        cursor = body["next_cursor"].as_str().map(str::to_owned);
        assert_eq!(
            body["truncated"],
            cursor.is_some(),
            "a cursor is given exactly when tests are left: {body}"
        );
        seen.push(body);
        assert!(seen.len() <= 10, "the cursor does not advance");
        if cursor.is_none() {
            return seen;
        }
    }
}

#[tokio::test]
async fn the_tests_come_a_page_at_a_time_with_a_cursor() {
    let srv = spawn_server().await;
    seed_dr(&srv).await;
    let (_, whole) = get(&srv.base, "/api/dr").await;

    let walked = pages(&srv.base, "/api/dr?limit=2").await;

    let counts: Vec<u64> = walked
        .iter()
        .map(|p| p["count"].as_u64().unwrap())
        .collect();
    assert_eq!(counts, [2, 2, 1]);
    let ids: Vec<String> = walked.iter().flat_map(test_ids).collect();
    assert_eq!(ids, test_ids(&whole), "every test once, in order");
    assert!(walked.iter().all(|page| page["total"] == 5));
    assert!(
        walked
            .iter()
            .all(|page| page["summary"] == whole["summary"]),
        "the summary counts the whole period on every page"
    );
    assert!(
        walked
            .iter()
            .all(|page| page["targets"] == whole["targets"]),
        "every page has the targets"
    );

    let walked = pages(&srv.base, "/api/dr?environment=prod&limit=3").await;
    let ids: Vec<String> = walked.iter().flat_map(test_ids).collect();
    assert_eq!(ids.len(), 4, "the filters hold on every page: {ids:?}");

    let (_, body) = get(&srv.base, "/api/dr?limit=5").await;
    assert_eq!(
        (&body["count"], &body["truncated"], &body["next_cursor"]),
        (&json!(5), &json!(false), &Value::Null),
        "a page that holds the last test is the last page"
    );
    let (_, body) = get(&srv.base, "/api/dr?limit=0").await;
    assert_eq!(body["count"], 1, "a limit below one is one: {body}");
    for broken in ["zz", "5b", "5b315d"] {
        let (status, body) = get(&srv.base, &format!("/api/dr?cursor={broken}")).await;
        assert_eq!(status, 400, "{broken}: {body}");
        assert_eq!(body["error"], "invalid cursor");
    }
    let (status, body) = get(&srv.base, "/api/dr?cursor=").await;
    assert_eq!(status, 200, "an empty cursor is none: {body}");
}

/// The house says which manual exercise types are DR tests. A house that
/// counts a tabletop as DR, and lists no other type, has its tabletop in
/// the view and not its failover.
#[tokio::test]
async fn the_house_says_which_manual_exercises_are_dr_tests() {
    let house = tempfile::TempDir::new().unwrap();
    let org_file = house.path().join("org.yaml");
    let manual_kinds = "score:\n  manual_kinds: {tabletop: dr}\n";
    std::fs::write(
        &org_file,
        format!("{}{manual_kinds}", crate::dora::REGISTER),
    )
    .unwrap();
    let srv = spawn_server_with_score_file(org_file).await;
    let at = now_ns() - 10 * DAY;
    let records = vec![
        exercise(
            "ledger tabletop",
            ExerciseType::Tabletop,
            "ledger",
            at,
            300.0,
        ),
        exercise(
            "catalog failover",
            ExerciseType::Failover,
            "catalog",
            at,
            300.0,
        ),
    ];
    exec_write(&srv, move |w| {
        for record in &records {
            let id = w.create_manual_draft(record).map_err(|e| e.to_string())?;
            w.submit_manual(&id, None, "carol")
                .map_err(|e| e.to_string())?;
            w.verify_manual(&id, "bob", None)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await;

    let (status, view) = get(&srv.base, "/api/dr").await;

    assert_eq!(status, 200, "{view}");
    let names: Vec<&str> = view["tests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["experiment_name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["ledger tabletop"]);
    assert_eq!(
        target_of(&view, "ledger")["last_test"]["origin"],
        "manual",
        "{view}"
    );
}

#[tokio::test]
async fn an_unknown_or_malformed_query_field_is_refused() {
    let srv = spawn_server().await;

    let (status, body) = get(&srv.base, "/api/dr?system=billing").await;
    assert_eq!(status, 400, "{body}");
    let error = body["error"].as_str().unwrap();
    assert!(error.contains("unknown field `system`"), "{body}");

    let (status, body) = get(&srv.base, "/api/dr?from=yesterday").await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"].as_str().unwrap().starts_with("invalid query"),
        "{body}"
    );
}
