//! Sign-offs of a manual record and the audit verify of records and
//! plans: `POST /api/manual/experiments/{id}/verify` and `/reject` are
//! sign-offs, the house sets how many approvals a type needs, and
//! `GET /api/manual/{experiments,plans}/{id}/audit/verify` finds a direct
//! edit or a deleted row in the lake.

use reqwest::Method;
use serde_json::{json, Value};

use crate::common::*;

const DAY: i64 = 86_400 * NS;

/// The test server with `manual_required_signoffs: {<signoffs>}` in the
/// `score:` section of its org file.
async fn spawn_with_signoffs(signoffs: &str) -> (TestServer, tempfile::TempDir) {
    let house = tempfile::TempDir::new().unwrap();
    let org_file = house.path().join("org.yaml");
    let score = format!("score:\n  manual_required_signoffs: {{{signoffs}}}\n");
    std::fs::write(&org_file, format!("{}{score}", crate::dora::REGISTER)).unwrap();
    (spawn_server_with_score_file(org_file).await, house)
}

/// A failover of payments-db with two steps, an action and a function
/// link, entered by `by`.
fn failover_body(by: &str) -> Value {
    json!({
        "experiment_name": "payments-db failover",
        "exercise_type": "failover",
        "executed_at_ns": now_ns() - 3600 * NS,
        "hypothesis": "The standby takes the writes within 900 s",
        "method": "Stop the primary and promote the standby",
        "outcome_status": "passed",
        "target_system": "payments-db",
        "target_environment": "production",
        "recovery_time_s": 600.0,
        "entered_by": by,
        "attestation": "I attest this record reflects the exercise as executed.",
        "rto_target_s": 900.0,
        "steps": [
            {"title": "Stop the primary", "actor": "alice", "result": "done"},
            {"title": "Promote the standby", "actor": "dan", "result": "done"}
        ],
        "actions": [{"title": "Automate the promotion", "owner": "dan"}],
        "links": [{"kind": "function", "linked_id": "cif-0001"}]
    })
}

async fn send(base: &str, method: Method, path: &str, body: &Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .request(method, format!("{base}{path}"))
        .json(body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

async fn post(base: &str, path: &str, body: &Value) -> (u16, Value) {
    send(base, Method::POST, path, body).await
}

/// Create `body` as a draft and submit it as alice; returns its id.
async fn submitted(base: &str, body: &Value) -> String {
    let (status, made) = post(base, "/api/manual/experiments", body).await;
    assert_eq!(status, 201, "{made}");
    let id = made["id"].as_str().unwrap().to_string();
    let path = format!("/api/manual/experiments/{id}/submit");
    let (status, done) = post(base, &path, &json!({"by": "alice"})).await;
    assert_eq!(status, 200, "{done}");
    id
}

/// Sign record `id` off as `reviewer`: `action` is `verify` or `reject`.
async fn sign(base: &str, id: &str, action: &str, reviewer: &str) -> (u16, Value) {
    let body = json!({"reviewer": reviewer, "note": format!("{reviewer} checked the logs")});
    post(
        base,
        &format!("/api/manual/experiments/{id}/{action}"),
        &body,
    )
    .await
}

async fn approve(base: &str, id: &str, reviewer: &str) {
    let (status, body) = sign(base, id, "verify", reviewer).await;
    assert_eq!(status, 200, "{reviewer}: {body}");
}

async fn detail(base: &str, id: &str) -> Value {
    let (status, detail) = get(base, &format!("/api/manual/experiments/{id}")).await;
    assert_eq!(status, 200, "{detail}");
    detail
}

/// The verdict of `GET /api/manual/<kind>/{id}/audit/verify`.
async fn verdict(base: &str, kind: &str, id: &str) -> Value {
    let (status, verdict) = get(base, &format!("/api/manual/{kind}/{id}/audit/verify")).await;
    assert_eq!(status, 200, "{verdict}");
    verdict
}

fn actions(detail: &Value) -> Vec<&str> {
    detail["audit"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect()
}

/// Whether a problem of `verdict` names row `row` of `table`, with
/// `field` when one is given.
fn names(verdict: &Value, table: &str, row: &str, field: Option<&str>) -> bool {
    verdict["problems"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["table"] == table && p["row"] == row && field.is_none_or(|f| p["field"] == f))
}

#[tokio::test]
async fn a_failover_stays_submitted_until_two_users_have_signed_it_off() {
    let (srv, _house) = spawn_with_signoffs("failover: 2").await;
    let base = srv.base.as_str();
    let id = submitted(base, &failover_body("alice")).await;

    // The first approval is a sign-off, not yet the verification.
    let (status, body) = sign(base, &id, "verify", "bob").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "submitted", "{body}");
    assert_eq!(body["signoffs_needed"], 1, "{body}");
    let first = detail(base, &id).await;
    assert_eq!(first["experiment"]["status"], "submitted");
    assert_eq!(first["experiment"]["reviewed_by"], Value::Null);
    assert_eq!(first["signoffs_required"], 2);
    assert_eq!(first["signoffs_needed"], 1);
    let signoffs = first["signoffs"].as_array().unwrap();
    assert_eq!(signoffs.len(), 1, "{first}");
    assert_eq!(signoffs[0]["signed_by"], "bob");
    assert_eq!(signoffs[0]["decision"], "approved");
    assert_eq!(signoffs[0]["subject_kind"], "record");
    assert_eq!(signoffs[0]["prev_hash"], Value::Null);
    assert_eq!(
        signoffs[0]["signed_hash"],
        first["experiment"]["content_hash"]
    );

    // The same user twice, and the user who entered the record, are
    // refused with the reason.
    for (action, reviewer, reason) in [
        ("verify", "bob", "has already signed off"),
        ("reject", "bob", "has already signed off"),
        ("verify", "alice", "must not have created"),
    ] {
        let (status, body) = sign(base, &id, action, reviewer).await;
        assert_eq!(status, 422, "{action} by {reviewer}: {body}");
        let error = body["error"].as_str().unwrap();
        assert!(error.contains(reason), "{action} by {reviewer}: {error}");
    }
    assert_eq!(detail(base, &id).await["signoffs"], first["signoffs"]);

    // The second approval, by another user, verifies the record.
    let (status, body) = sign(base, &id, "verify", "carol").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "verified", "{body}");
    assert_eq!(body["signoffs_needed"], 0, "{body}");
    let done = detail(base, &id).await;
    assert_eq!(done["experiment"]["status"], "verified");
    assert_eq!(done["experiment"]["reviewed_by"], "carol");
    assert_eq!(done["signoffs_needed"], 0);
    let signoffs = done["signoffs"].as_array().unwrap();
    let signers: Vec<&str> = signoffs
        .iter()
        .map(|s| s["signed_by"].as_str().unwrap())
        .collect();
    assert_eq!(signers, ["bob", "carol"]);
    assert_eq!(signoffs[1]["prev_hash"], signoffs[0]["link_hash"]);
    assert_eq!(signoffs[1]["signed_hash"], signoffs[0]["signed_hash"]);
    assert_eq!(actions(&done), ["create", "submit", "signoff", "verify"]);
    let verdict = verdict(base, "experiments", &id).await;
    assert_eq!(verdict["ok"], true, "{verdict}");
}

#[tokio::test]
async fn a_reject_after_one_approval_rejects_the_record() {
    let (srv, _house) = spawn_with_signoffs("failover: 2").await;
    let base = srv.base.as_str();
    let id = submitted(base, &failover_body("alice")).await;
    approve(base, &id, "bob").await;
    let (status, body) = sign(base, &id, "reject", "carol").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "rejected", "{body}");
    let done = detail(base, &id).await;
    assert_eq!(done["experiment"]["status"], "rejected");
    assert_eq!(done["experiment"]["reviewed_by"], "carol");
    let decisions: Vec<&str> = done["signoffs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["decision"].as_str().unwrap())
        .collect();
    assert_eq!(decisions, ["approved", "rejected"]);
    let (status, _) = sign(base, &id, "verify", "dan").await;
    assert_eq!(status, 409, "a rejected record takes no more sign-offs");
    assert_eq!(verdict(base, "experiments", &id).await["ok"], true);
}

#[tokio::test]
async fn one_approval_verifies_a_record_with_the_default_house_parameters() {
    let srv = spawn_server().await;
    let base = srv.base.as_str();
    let id = submitted(base, &failover_body("alice")).await;
    let pending = detail(base, &id).await;
    assert_eq!(pending["signoffs_required"], 1);
    assert_eq!(pending["signoffs_needed"], 1);
    let (status, body) = sign(base, &id, "verify", "bob").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["status"], "verified", "{body}");
    let done = detail(base, &id).await;
    assert_eq!(done["experiment"]["status"], "verified");
    assert_eq!(done["experiment"]["reviewed_by"], "bob");
    assert_eq!(done["signoffs"].as_array().unwrap().len(), 1);
    assert_eq!(done["signoffs_needed"], 0);
    assert_eq!(actions(&done), ["create", "submit", "verify"]);
}

/// A plan of the failover, due in 30 days, edited once.
async fn edited_plan(base: &str) -> String {
    let plan = json!({
        "name": "payments-db failover",
        "exercise_type": "failover",
        "target_system": "payments-db",
        "target_environment": "production",
        "owner": "alice",
        "due_at_ns": now_ns() + 30 * DAY,
        "interval_days": 180,
        "rto_target_s": 900.0,
        "created_by": "alice"
    });
    let (status, made) = post(base, "/api/manual/plans", &plan).await;
    assert_eq!(status, 201, "{made}");
    let id = made["id"].as_str().unwrap().to_string();
    let edit = json!({"rto_target_s": 600.5, "scope": "Promote the standby", "updated_by": "bob"});
    let (status, body) = send(
        base,
        Method::PATCH,
        &format!("/api/manual/plans/{id}"),
        &edit,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    id
}

#[tokio::test]
async fn verify_is_ok_on_an_untouched_record_and_plan() {
    let (srv, _house) = spawn_with_signoffs("failover: 2").await;
    let base = srv.base.as_str();
    let plan = edited_plan(base).await;
    let mut body = failover_body("alice");
    body["plan_id"] = json!(plan);
    let (status, made) = post(base, "/api/manual/experiments", &body).await;
    assert_eq!(status, 201, "{made}");
    let id = made["id"].as_str().unwrap().to_string();
    let link = json!({"kind": "url", "uri": "https://wiki.example/failover", "added_by": "alice"});
    let (status, _) = post(
        base,
        &format!("/api/manual/experiments/{id}/attachments"),
        &link,
    )
    .await;
    assert_eq!(status, 201);
    let (status, _) = post(
        base,
        &format!("/api/manual/experiments/{id}/submit"),
        &json!({"by": "alice"}),
    )
    .await;
    assert_eq!(status, 200);
    approve(base, &id, "bob").await;
    approve(base, &id, "carol").await;

    let record = verdict(base, "experiments", &id).await;
    assert_eq!(record["experiment_id"], id.as_str());
    assert_eq!(record["ok"], true, "{record}");
    assert_eq!(record["chain_valid"], true, "{record}");
    assert_eq!(record["hash_version"], 2);
    assert_eq!(record["legacy_rows"], 0);
    assert_eq!(record["problems"], json!([]));

    // The plan was created, edited and fulfilled; its next plan created.
    let (_, plans) = get(base, "/api/manual/plans").await;
    let ids: Vec<&str> = plans["plans"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids.len(), 2, "{plans}");
    for plan in ids {
        let verdict = verdict(base, "plans", plan).await;
        assert_eq!(verdict["plan_id"], plan);
        assert_eq!(verdict["ok"], true, "{verdict}");
        assert_eq!(verdict["problems"], json!([]), "{verdict}");
    }
}

/// Run `sql` straight on the lake, past every rule of the API.
async fn tamper(srv: &TestServer, sql: String) {
    exec_write(srv, move |w| {
        w.execute(&sql, []).map(|_| ()).map_err(|e| e.to_string())
    })
    .await;
}

#[tokio::test]
async fn verify_names_the_row_after_a_direct_edit_or_a_deleted_row() {
    let (srv, _house) = spawn_with_signoffs("failover: 2").await;
    let base = srv.base.as_str();
    // Each case: the statement, given the record and its detail, and the
    // table, row and field the verdict must name.
    type Case = fn(&str, &Value) -> (String, &'static str, String, Option<&'static str>);
    let cases: [(&str, Case); 9] = [
        ("reviewed_by", |id, _| {
            (
                format!("UPDATE manual_experiments SET reviewed_by = 'mallory' WHERE id = '{id}'"),
                "manual_experiments",
                id.to_string(),
                Some("reviewed_by"),
            )
        }),
        ("a step", |id, _| {
            (
                format!(
                    "UPDATE manual_exercise_steps SET result = 'failed' \
                     WHERE experiment_id = '{id}' AND ordinal = 2"
                ),
                "manual_experiments",
                id.to_string(),
                None,
            )
        }),
        ("an action definition", |id, _| {
            (
                format!(
                    "UPDATE manual_action_items SET owner = 'mallory' WHERE experiment_id = '{id}'"
                ),
                "manual_experiments",
                id.to_string(),
                None,
            )
        }),
        ("a deleted audit row", |id, detail| {
            // The submit row goes; the sign-off row after it breaks.
            let audit = detail["audit"].as_array().unwrap();
            let after = audit[2]["id"].as_str().unwrap().to_string();
            (
                format!(
                    "DELETE FROM manual_experiment_audit \
                     WHERE experiment_id = '{id}' AND action = 'submit'"
                ),
                "manual_experiment_audit",
                after,
                None,
            )
        }),
        ("the last audit row deleted", |id, detail| {
            // The verify row goes; the sign-off row is last and did not
            // leave the stored seal.
            let row = detail["audit"][2]["id"].as_str().unwrap().to_string();
            (
                format!(
                    "DELETE FROM manual_experiment_audit \
                     WHERE experiment_id = '{id}' AND action = 'verify'"
                ),
                "manual_experiment_audit",
                row,
                Some("new_hash"),
            )
        }),
        ("an audit row", |_, detail| {
            let row = detail["audit"][3]["id"].as_str().unwrap().to_string();
            (
                format!(
                    "UPDATE manual_experiment_audit SET changed_by = 'mallory' WHERE id = '{row}'"
                ),
                "manual_experiment_audit",
                row,
                None,
            )
        }),
        ("a sign-off row", |_, detail| {
            let row = detail["signoffs"][0]["id"].as_str().unwrap().to_string();
            (
                format!("UPDATE manual_signoffs SET signed_by = 'mallory' WHERE id = '{row}'"),
                "manual_signoffs",
                row,
                None,
            )
        }),
        ("a deleted sign-off row", |_, detail| {
            // The last sign-off goes; the verify row still names it.
            let gone = detail["signoffs"][1]["id"].as_str().unwrap().to_string();
            let row = detail["audit"][3]["id"].as_str().unwrap().to_string();
            (
                format!("DELETE FROM manual_signoffs WHERE id = '{gone}'"),
                "manual_experiment_audit",
                row,
                Some("diff"),
            )
        }),
        ("a sign-off of another seal", |_, detail| {
            let row = detail["signoffs"][1]["id"].as_str().unwrap().to_string();
            (
                format!(
                    "UPDATE manual_signoffs SET signed_hash = repeat('0', 64) WHERE id = '{row}'"
                ),
                "manual_signoffs",
                row,
                Some("signed_hash"),
            )
        }),
    ];
    for (what, case) in cases {
        let id = submitted(base, &failover_body("alice")).await;
        approve(base, &id, "bob").await;
        approve(base, &id, "carol").await;
        assert_eq!(
            verdict(base, "experiments", &id).await["ok"],
            true,
            "{what}"
        );
        let (sql, table, row, field) = case(&id, &detail(base, &id).await);
        tamper(&srv, sql).await;
        let verdict = verdict(base, "experiments", &id).await;
        assert_eq!(verdict["ok"], false, "{what}: {verdict}");
        assert_eq!(verdict["chain_valid"], false, "{what}: {verdict}");
        assert!(names(&verdict, table, &row, field), "{what}: {verdict}");
    }

    // A record with one of its two approvals: its last audit row is the
    // sign-off row and leaves the seal as it was, so only the sign-off it
    // names shows that the row was deleted.
    let id = submitted(base, &failover_body("alice")).await;
    approve(base, &id, "bob").await;
    let signoff = detail(base, &id).await["signoffs"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    tamper(
        &srv,
        format!(
            "DELETE FROM manual_experiment_audit \
             WHERE experiment_id = '{id}' AND action = 'signoff'"
        ),
    )
    .await;
    let found = verdict(base, "experiments", &id).await;
    assert_eq!(found["ok"], false, "{found}");
    assert!(names(&found, "manual_signoffs", &signoff, None), "{found}");

    // A plan: a direct edit of a column, and a deleted audit row.
    for (what, sql, table) in [
        (
            "a plan column",
            "UPDATE manual_exercise_plans SET due_at_ns = due_at_ns + 1 WHERE id = '{id}'",
            "manual_exercise_plans",
        ),
        (
            "a deleted plan audit row",
            "DELETE FROM manual_plan_audit WHERE plan_id = '{id}' AND action = 'create'",
            "manual_plan_audit",
        ),
    ] {
        let plan = edited_plan(base).await;
        assert_eq!(verdict(base, "plans", &plan).await["ok"], true, "{what}");
        let (_, before) = get(base, &format!("/api/manual/plans/{plan}")).await;
        let row = match table {
            "manual_plan_audit" => before["audit"][1]["id"].as_str().unwrap().to_string(),
            _ => plan.clone(),
        };
        tamper(&srv, sql.replace("{id}", &plan)).await;
        let verdict = verdict(base, "plans", &plan).await;
        assert_eq!(verdict["ok"], false, "{what}: {verdict}");
        assert!(names(&verdict, table, &row, None), "{what}: {verdict}");
    }
}

#[tokio::test]
async fn verify_keeps_to_the_scopes_of_the_caller() {
    let srv = spawn_server().await;
    let base = srv.base.as_str();
    let admin = add_scoped_token(&srv, "verify-admin", "admin", &[]).await;
    let scoped = add_scoped_token(&srv, "verify-scoped", "viewer", &["staging"]).await;
    let (status, made) =
        post_auth(base, "/api/manual/experiments", &admin, failover_body("x")).await;
    assert_eq!(status, 201, "{made}");
    let record = made["id"].as_str().unwrap().to_string();
    let plan = json!({
        "name": "payments-db failover", "exercise_type": "failover",
        "target_environment": "production", "owner": "alice",
        "due_at_ns": now_ns() + DAY
    });
    let (status, made) = post_auth(base, "/api/manual/plans", &admin, plan).await;
    assert_eq!(status, 201, "{made}");
    let plan = made["id"].as_str().unwrap().to_string();
    for path in [
        format!("/api/manual/experiments/{record}/audit/verify"),
        format!("/api/manual/plans/{plan}/audit/verify"),
    ] {
        let (status, body) = get_auth(base, &path, &admin).await;
        assert_eq!(status, 200, "{path}: {body}");
        assert_eq!(body["ok"], true, "{path}: {body}");
        let (status, _) = get_auth(base, &path, &scoped).await;
        assert_eq!(status, 404, "{path} outside the scopes");
    }
    for path in [
        "/api/manual/experiments/no-such-record/audit/verify",
        "/api/manual/plans/no-such-plan/audit/verify",
    ] {
        assert_eq!(get_auth(base, path, &admin).await.0, 404, "{path}");
    }
}
