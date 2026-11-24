//! `/api/manual/plans`: planned manual exercises, their audit trail, the
//! verified record that fulfils a plan, and the overdue finding of
//! `/api/risks`.

use reqwest::Method;
use serde_json::{json, Value};

use crate::common::*;

const DAY: i64 = 86_400 * NS;

/// A plan of a `exercise_type` exercise of `database` in `production`,
/// owned by alice and due `due_in_days` from now.
fn plan_body(exercise_type: &str, due_in_days: i64) -> Value {
    json!({
        "name": "database failover",
        "exercise_type": exercise_type,
        "target_system": "database",
        "target_environment": "production",
        "function_id": "cif-0001",
        "scope": "Promote the standby and move the writers",
        "owner": "alice",
        "due_at_ns": now_ns() + due_in_days * DAY,
        "rto_target_s": 900.0,
        "rpo_target_s": 60.0,
        "created_by": "alice"
    })
}

/// A failover of `database` in `production` against the function
/// `cif-0001`, run an hour ago, entered by carol and naming `plan_id`.
fn record_body(plan_id: &str) -> Value {
    json!({
        "experiment_name": "database failover",
        "exercise_type": "failover",
        "executed_at_ns": now_ns() - 3600 * NS,
        "hypothesis": "The standby takes the writes within 900 s",
        "method": "Stop the primary and promote the standby",
        "outcome_status": "passed",
        "target_system": "database",
        "target_environment": "production",
        "recovery_time_s": 600.0,
        "entered_by": "carol",
        "attestation": "I attest this record reflects the exercise as executed.",
        "links": [{"kind": "function", "linked_id": "cif-0001"}],
        "plan_id": plan_id
    })
}

/// Send `body` to `path` with `method`; returns the status and the body as
/// text.
async fn send_text(
    base: &str,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: &Value,
) -> (u16, String) {
    let mut req = reqwest::Client::new()
        .request(method, format!("{base}{path}"))
        .json(body);
    if let Some(token) = token {
        req = req.bearer_auth(token);
    }
    let resp = req.send().await.unwrap();
    (resp.status().as_u16(), resp.text().await.unwrap())
}

/// Send `body` to `path` with `method`; returns the status and the JSON
/// body.
async fn send(base: &str, method: Method, path: &str, body: &Value) -> (u16, Value) {
    let (status, text) = send_text(base, method, path, None, body).await;
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

async fn create_plan(base: &str, body: &Value) -> String {
    let (status, made) = send(base, Method::POST, "/api/manual/plans", body).await;
    assert_eq!(status, 201, "{made}");
    made["id"].as_str().unwrap().to_string()
}

async fn plan_of(base: &str, id: &str) -> Value {
    let (status, detail) = get(base, &format!("/api/manual/plans/{id}")).await;
    assert_eq!(status, 200, "{detail}");
    detail
}

fn actions(detail: &Value) -> Vec<String> {
    detail["audit"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action"].as_str().unwrap().to_string())
        .collect()
}

/// Each audit row starts from the seal the row before it left, and the
/// last one left the seal the plan holds now.
fn assert_chained(detail: &Value) {
    let audit = detail["audit"].as_array().unwrap();
    assert_eq!(audit[0]["prev_hash"], Value::Null);
    for w in audit.windows(2) {
        assert_eq!(w[1]["prev_hash"], w[0]["new_hash"], "{detail}");
    }
    assert_eq!(
        audit.last().unwrap()["new_hash"],
        detail["plan"]["content_hash"]
    );
}

fn plan_ids(list: &Value) -> Vec<String> {
    list["plans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap().to_string())
        .collect()
}

/// The overdue findings of a risk report, by plan id.
fn overdue(report: &Value) -> Vec<&Value> {
    report["risks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["rule"] == "manual_exercise_overdue")
        .collect()
}

fn overdue_plans(report: &Value) -> Vec<String> {
    overdue(report)
        .iter()
        .map(|r| r["subject"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn a_plan_is_created_edited_listed_and_cancelled_over_the_api() {
    let srv = spawn_server().await;
    let base = srv.base.clone();

    // A DR plan sent without an interval is due every 365 days; a tabletop
    // gets no interval; an interval sent is kept.
    let failover = create_plan(&base, &plan_body("failover", 30)).await;
    let mut tabletop = plan_body("tabletop", 60);
    tabletop["target_system"] = json!("ledger");
    let tabletop = create_plan(&base, &tabletop).await;
    let mut restore = plan_body("restore", 90);
    restore["interval_days"] = json!(30);
    let restore = create_plan(&base, &restore).await;

    let created = plan_of(&base, &failover).await;
    let plan = &created["plan"];
    assert_eq!(plan["status"], "planned");
    assert_eq!(plan["interval_days"], 365, "{created}");
    assert_eq!(plan["owner"], "alice");
    assert_eq!(plan["function_id"], "cif-0001");
    assert_eq!(plan["created_by"], "alice");
    assert_eq!(actions(&created), ["create"]);
    assert_chained(&created);
    assert_eq!(
        plan_of(&base, &tabletop).await["plan"]["interval_days"],
        Value::Null
    );
    assert_eq!(plan_of(&base, &restore).await["plan"]["interval_days"], 30);

    let due = now_ns() + 45 * DAY;
    let patch = json!({"due_at_ns": due, "scope": null, "updated_by": "bob"});
    let path = format!("/api/manual/plans/{failover}");
    let (status, body) = send(&base, Method::PATCH, &path, &patch).await;
    assert_eq!(status, 200, "{body}");
    let edited = plan_of(&base, &failover).await;
    assert_eq!(edited["plan"]["due_at_ns"], due);
    assert_eq!(edited["plan"]["scope"], Value::Null);
    assert_eq!(edited["plan"]["owner"], "alice", "kept: not in the edit");
    assert_eq!(edited["plan"]["updated_by"], "bob");
    assert_eq!(actions(&edited), ["create", "edit"]);
    assert_chained(&edited);

    let (_, all) = get(&base, "/api/manual/plans").await;
    assert_eq!(
        plan_ids(&all),
        [failover.as_str(), tabletop.as_str(), restore.as_str()]
    );
    let (_, ledger) = get(&base, "/api/manual/plans?target=ledger").await;
    assert_eq!(plan_ids(&ledger), [tabletop.as_str()]);
    let before = now_ns() + 50 * DAY;
    let (_, soon) = get(&base, &format!("/api/manual/plans?due_before={before}")).await;
    assert_eq!(plan_ids(&soon), [failover.as_str()]);

    let cancel = json!({"by": "carol", "reason": "merged into the Q1 drill"});
    let cancel_path = format!("/api/manual/plans/{failover}/cancel");
    let (status, body) = send(&base, Method::POST, &cancel_path, &cancel).await;
    assert_eq!(status, 200, "{body}");
    let cancelled = plan_of(&base, &failover).await;
    assert_eq!(cancelled["plan"]["status"], "cancelled");
    assert_eq!(
        cancelled["audit"][2]["diff"]["reason"],
        "merged into the Q1 drill"
    );
    assert_chained(&cancelled);
    let (_, planned) = get(&base, "/api/manual/plans?status=planned").await;
    assert_eq!(plan_ids(&planned), [tabletop.as_str(), restore.as_str()]);

    // A cancelled plan takes no edit and no second cancel.
    let (status, body) = send(&base, Method::PATCH, &path, &patch).await;
    assert_eq!(status, 409, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("cannot edit a plan in status 'cancelled'"),
        "{body}"
    );
    let (status, _) = send(&base, Method::POST, &cancel_path, &cancel).await;
    assert_eq!(status, 409);
    assert_eq!(
        plan_of(&base, &failover).await["audit"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    // An unknown plan is 404 on every route; bad content is 400.
    let (status, _) = get(&base, "/api/manual/plans/no-such-plan").await;
    assert_eq!(status, 404);
    let (status, _) = send(
        &base,
        Method::PATCH,
        "/api/manual/plans/no-such-plan",
        &patch,
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) = send(
        &base,
        Method::POST,
        "/api/manual/plans/no-such-plan/cancel",
        &cancel,
    )
    .await;
    assert_eq!(status, 404);
    let mut bad = plan_body("failover", 30);
    bad["interval_days"] = json!(0);
    let (status, body) = send(&base, Method::POST, "/api/manual/plans", &bad).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("'interval_days'"),
        "{body}"
    );
    let mut bad = plan_body("teleport", 30);
    bad["owner"] = json!("alice");
    let (status, body) = send(&base, Method::POST, "/api/manual/plans", &bad).await;
    assert_eq!(status, 400, "{body}");
}

#[tokio::test]
async fn every_plan_body_refuses_a_field_it_does_not_know() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let id = create_plan(&base, &plan_body("failover", 30)).await;

    let mut create = plan_body("failover", 30);
    create["due"] = json!(1);
    let cases = [
        (Method::POST, "/api/manual/plans".to_string(), create, "due"),
        (
            Method::PATCH,
            format!("/api/manual/plans/{id}"),
            json!({"owner": "bob", "colour": "red"}),
            "colour",
        ),
        (
            Method::POST,
            format!("/api/manual/plans/{id}/cancel"),
            json!({"by": "carol", "note": "dropped"}),
            "note",
        ),
    ];
    for (method, path, body, field) in cases {
        let (status, text) = send_text(&base, method, &path, None, &body).await;
        assert_eq!(status, 422, "{path} {field}: {text}");
        assert!(
            text.contains(&format!("unknown field `{field}`")),
            "{path}: {text}"
        );
    }

    // Nothing of the refused bodies was stored or changed.
    let (_, all) = get(&base, "/api/manual/plans").await;
    assert_eq!(plan_ids(&all), [id.as_str()]);
    let detail = plan_of(&base, &id).await;
    assert_eq!(detail["plan"]["status"], "planned");
    assert_eq!(detail["plan"]["owner"], "alice");
    assert_eq!(actions(&detail), ["create"]);
}

#[tokio::test]
async fn a_plan_due_yesterday_is_overdue_until_a_verified_record_fulfils_it() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let plan = create_plan(&base, &plan_body("failover", -1)).await;

    let (status, risks) = get(&base, "/api/risks").await;
    assert_eq!(status, 200, "{risks}");
    assert_eq!(overdue_plans(&risks), [plan.as_str()], "{risks}");
    let finding = overdue(&risks)[0];
    assert_eq!(finding["severity"], "medium");
    assert_eq!(finding["target"], "database");
    assert_eq!(finding["environment"], "production");
    assert!(
        finding["title"].as_str().unwrap().contains("overdue"),
        "{finding}"
    );

    // A rejected record leaves the plan planned, and so overdue.
    let (status, made) = send(
        &base,
        Method::POST,
        "/api/manual/experiments",
        &record_body(&plan),
    )
    .await;
    assert_eq!(status, 201, "{made}");
    let rejected = made["id"].as_str().unwrap().to_string();
    let by = json!({"by": "carol"});
    let path = format!("/api/manual/experiments/{rejected}/submit");
    assert_eq!(send(&base, Method::POST, &path, &by).await.0, 200);
    let review = json!({"reviewer": "bob", "note": "no promotion log"});
    let path = format!("/api/manual/experiments/{rejected}/reject");
    assert_eq!(send(&base, Method::POST, &path, &review).await.0, 200);
    assert_eq!(plan_of(&base, &plan).await["plan"]["status"], "planned");
    let (_, risks) = get(&base, "/api/risks").await;
    assert_eq!(overdue_plans(&risks), [plan.as_str()]);

    // A verified record fulfils it.
    let record = record_body(&plan);
    let ran_at = record["executed_at_ns"].as_i64().unwrap();
    let (status, made) = send(&base, Method::POST, "/api/manual/experiments", &record).await;
    assert_eq!(status, 201, "{made}");
    let verified = made["id"].as_str().unwrap().to_string();
    let path = format!("/api/manual/experiments/{verified}/submit");
    assert_eq!(send(&base, Method::POST, &path, &by).await.0, 200);
    let review = json!({"reviewer": "bob", "note": "promotion log checked"});
    let path = format!("/api/manual/experiments/{verified}/verify");
    let (status, body) = send(&base, Method::POST, &path, &review).await;
    assert_eq!(status, 200, "{body}");

    let fulfilled = plan_of(&base, &plan).await;
    assert_eq!(fulfilled["plan"]["status"], "fulfilled");
    assert_eq!(actions(&fulfilled), ["create", "fulfil"]);
    assert_chained(&fulfilled);
    let diff = &fulfilled["audit"][1]["diff"];
    assert_eq!(diff["record_id"], verified.as_str());
    let next = diff["next_plan_id"].as_str().unwrap().to_string();
    let next = plan_of(&base, &next).await;
    assert_eq!(next["plan"]["status"], "planned");
    assert_eq!(next["plan"]["due_at_ns"], ran_at + 365 * DAY);
    assert_eq!(next["plan"]["interval_days"], 365);
    assert_eq!(next["plan"]["owner"], "alice");
    assert_eq!(next["plan"]["target_system"], "database");
    assert_eq!(actions(&next), ["create"]);

    let (_, risks) = get(&base, "/api/risks").await;
    assert!(overdue(&risks).is_empty(), "{risks}");
}

#[tokio::test]
async fn a_cancelled_plan_is_never_overdue() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let plan = create_plan(&base, &plan_body("restore", -40)).await;
    let (_, risks) = get(&base, "/api/risks").await;
    assert_eq!(overdue_plans(&risks), [plan.as_str()]);
    assert_eq!(overdue(&risks)[0]["severity"], "high", "40 days past due");

    let path = format!("/api/manual/plans/{plan}/cancel");
    let (status, body) = send(&base, Method::POST, &path, &json!({"by": "alice"})).await;
    assert_eq!(status, 200, "{body}");
    let (_, risks) = get(&base, "/api/risks").await;
    assert!(overdue(&risks).is_empty(), "{risks}");
}

#[tokio::test]
async fn a_record_naming_a_plan_it_cannot_fulfil_is_refused_with_422() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let plan = create_plan(&base, &plan_body("failover", 30)).await;
    let dropped = create_plan(&base, &plan_body("failover", 30)).await;
    let path = format!("/api/manual/plans/{dropped}/cancel");
    assert_eq!(
        send(&base, Method::POST, &path, &json!({"by": "alice"}))
            .await
            .0,
        200
    );

    let mut restore = record_body(&plan);
    restore["exercise_type"] = json!("restore");
    let cases = [
        (
            "experiments",
            record_body("no-such-plan"),
            "plan 'no-such-plan' does not exist",
        ),
        (
            "experiments",
            restore,
            "plans a failover exercise, not a restore",
        ),
        (
            "experiments",
            record_body(&dropped),
            "is cancelled, not planned",
        ),
        (
            "import",
            json!({"records": [record_body("no-such-plan")]}),
            "does not exist",
        ),
    ];
    for (path, body, needle) in cases {
        let path = format!("/api/manual/{path}");
        let (status, body) = send(&base, Method::POST, &path, &body).await;
        assert_eq!(status, 422, "{body}");
        assert!(body["error"].as_str().unwrap().contains(needle), "{body}");
    }
    let (_, records) = get(&base, "/api/manual/experiments").await;
    assert!(records["records"].as_array().unwrap().is_empty());

    // The plan is checked again at sign-off.
    let (status, made) = send(
        &base,
        Method::POST,
        "/api/manual/experiments",
        &record_body(&plan),
    )
    .await;
    assert_eq!(status, 201, "{made}");
    let id = made["id"].as_str().unwrap().to_string();
    let path = format!("/api/manual/experiments/{id}/submit");
    assert_eq!(
        send(&base, Method::POST, &path, &json!({"by": "carol"}))
            .await
            .0,
        200
    );
    let path = format!("/api/manual/plans/{plan}/cancel");
    assert_eq!(
        send(&base, Method::POST, &path, &json!({"by": "alice"}))
            .await
            .0,
        200
    );
    let path = format!("/api/manual/experiments/{id}/verify");
    let (status, body) = send(&base, Method::POST, &path, &json!({"reviewer": "bob"})).await;
    assert_eq!(status, 422, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("is cancelled"),
        "{body}"
    );
    let (_, detail) = get(&base, &format!("/api/manual/experiments/{id}")).await;
    assert_eq!(detail["experiment"]["status"], "submitted");
}

#[tokio::test]
async fn a_record_fulfils_a_plan_only_when_it_tests_what_the_plan_planned() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let plan = create_plan(&base, &plan_body("failover", 30)).await;

    // A staging record may not fulfil a production plan.
    let mut staging = record_body(&plan);
    staging["target_environment"] = json!("staging");
    let (status, body) = send(&base, Method::POST, "/api/manual/experiments", &staging).await;
    assert_eq!(status, 422, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("plans target_environment 'production', the record has 'staging'"),
        "{body}"
    );
    let (_, records) = get(&base, "/api/manual/experiments").await;
    assert!(records["records"].as_array().unwrap().is_empty());

    // The plan moves to another environment after the record was drafted:
    // the verify is refused and both stay as they were.
    let (status, made) = send(
        &base,
        Method::POST,
        "/api/manual/experiments",
        &record_body(&plan),
    )
    .await;
    assert_eq!(status, 201, "{made}");
    let id = made["id"].as_str().unwrap().to_string();
    let path = format!("/api/manual/experiments/{id}/submit");
    assert_eq!(
        send(&base, Method::POST, &path, &json!({"by": "carol"}))
            .await
            .0,
        200
    );
    let moved = json!({"target_environment": "dr-site", "updated_by": "alice"});
    let path = format!("/api/manual/plans/{plan}");
    assert_eq!(send(&base, Method::PATCH, &path, &moved).await.0, 200);
    let path = format!("/api/manual/experiments/{id}/verify");
    let (status, body) = send(&base, Method::POST, &path, &json!({"reviewer": "bob"})).await;
    assert_eq!(status, 422, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .contains("plans target_environment 'dr-site', the record has 'production'"),
        "{body}"
    );
    let (_, detail) = get(&base, &format!("/api/manual/experiments/{id}")).await;
    assert_eq!(detail["experiment"]["status"], "submitted");
    assert_eq!(plan_of(&base, &plan).await["plan"]["status"], "planned");
}

/// The house parameters say which types are DR: a house that counts a
/// tabletop as DR, and lists no other type, gives a tabletop plan the
/// yearly interval and a failover plan none.
#[tokio::test]
async fn a_house_that_counts_a_tabletop_as_dr_gives_its_plans_a_yearly_interval() {
    let house = tempfile::TempDir::new().unwrap();
    let org_file = house.path().join("org.yaml");
    let manual_kinds = "score:\n  manual_kinds: {tabletop: dr}\n";
    std::fs::write(
        &org_file,
        format!("{}{manual_kinds}", crate::dora::REGISTER),
    )
    .unwrap();
    let srv = spawn_server_with_score_file(org_file).await;
    let tabletop = create_plan(&srv.base, &plan_body("tabletop", 30)).await;
    let failover = create_plan(&srv.base, &plan_body("failover", 30)).await;
    assert_eq!(
        plan_of(&srv.base, &tabletop).await["plan"]["interval_days"],
        365
    );
    assert_eq!(
        plan_of(&srv.base, &failover).await["plan"]["interval_days"],
        Value::Null
    );
}

#[tokio::test]
async fn plans_keep_to_the_scopes_of_the_caller() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let admin = add_scoped_token(&srv, "plan-admin", "admin", &[]).await;
    let scoped = add_scoped_token(&srv, "plan-scoped", "approver", &["staging"]).await;
    let (status, made) = post_auth(
        &base,
        "/api/manual/plans",
        &admin,
        plan_body("failover", -1),
    )
    .await;
    assert_eq!(status, 201, "{made}");
    let prod = made["id"].as_str().unwrap().to_string();

    let (status, list) = get_auth(&base, "/api/manual/plans", &scoped).await;
    assert_eq!(status, 200, "{list}");
    assert!(plan_ids(&list).is_empty(), "production plan leaked: {list}");
    let (status, _) = get_auth(&base, &format!("/api/manual/plans/{prod}"), &scoped).await;
    assert_eq!(status, 404);
    let (status, _) = send_text(
        &base,
        Method::PATCH,
        &format!("/api/manual/plans/{prod}"),
        Some(&scoped),
        &json!({"owner": "mallory"}),
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) = post_auth(
        &base,
        &format!("/api/manual/plans/{prod}/cancel"),
        &scoped,
        json!({}),
    )
    .await;
    assert_eq!(status, 404);
    let (status, _) = post_auth(
        &base,
        "/api/manual/plans",
        &scoped,
        plan_body("failover", 30),
    )
    .await;
    assert_eq!(status, 403);
    let (_, risks) = get_auth(&base, "/api/risks", &scoped).await;
    assert!(overdue(&risks).is_empty(), "{risks}");
    let (_, risks) = get_auth(&base, "/api/risks", &admin).await;
    assert_eq!(overdue_plans(&risks), [prod.as_str()]);

    // A record in the caller's scope may not name a plan outside it.
    let mut record = record_body(&prod);
    record["target_environment"] = json!("staging");
    let (status, body) = post_auth(&base, "/api/manual/experiments", &scoped, record).await;
    assert_eq!(status, 422, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("does not exist"),
        "{body}"
    );

    let mut staging = plan_body("failover", 30);
    staging["target_environment"] = json!("staging");
    let (status, made) = post_auth(&base, "/api/manual/plans", &scoped, staging).await;
    assert_eq!(status, 201, "{made}");
    let staging = made["id"].as_str().unwrap();
    let (_, list) = get_auth(&base, "/api/manual/plans", &scoped).await;
    assert_eq!(plan_ids(&list), [staging]);
    let (_, detail) = get_auth(&base, &format!("/api/manual/plans/{staging}"), &scoped).await;
    assert_eq!(detail["plan"]["created_by"], "plan-scoped", "{detail}");
}
