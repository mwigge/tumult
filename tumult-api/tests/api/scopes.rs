use crate::common::*;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// RBAC roles and env scopes on the mutation endpoints: dry-run is gated at
// Operator (previewing evaluates daemon-side configuration), and run /
// schedule / gameday launches reject an `env` outside the principal's
// scopes (the same rule the reads already apply).

pub(crate) const SCOPE_TOON: &str = "
title: scoped launch test experiment
method[1]:
  - name: action-1
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
rollbacks[1]:
  - name: rollback-1
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
";

const GAMEDAY: &str = "
title: scoped campaign
experiments[1]:
  - path: a.toon
    compliance_maps[0]:
";

/// Register SCOPE_TOON with an operator token; returns its registry id.
async fn register_def(base: &str, token: &str) -> String {
    let (status, body) = post_auth(
        base,
        "/api/runs/validate",
        token,
        json!({"toon": SCOPE_TOON}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["valid"], true, "{body}");
    body["registry_id"].as_str().unwrap().to_string()
}

/// `POST /api/runs/dry-run` remains Operator-only; its display artifact
/// masks server-derived values. A Viewer is refused (403).
#[tokio::test]
async fn dry_run_requires_operator() {
    let srv = spawn_server().await;
    let viewer = add_scoped_token(&srv, "viewer", "viewer", &[]).await;
    let operator = add_scoped_token(&srv, "operator", "operator", &[]).await;
    let registry_id = register_def(&srv.base, &operator).await;

    let (status, body) = post_auth(
        &srv.base,
        "/api/runs/dry-run",
        &viewer,
        json!({"registry_id": registry_id}),
    )
    .await;
    assert_eq!(status, 403, "{body}");

    let (status, body) = post_auth(
        &srv.base,
        "/api/runs/dry-run",
        &operator,
        json!({"registry_id": registry_id}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["valid"], true, "{body}");
}

/// A scoped operator launching a run into an environment outside its scopes
/// is a 403; an in-scope launch is accepted.
#[tokio::test]
async fn create_run_rejects_env_outside_the_principals_scopes() {
    let srv = spawn_server().await;
    let token = add_scoped_token(&srv, "run-op", "operator", &["staging"]).await;
    let registry_id = register_def(&srv.base, &token).await;

    let (status, body) = post_auth(
        &srv.base,
        "/api/runs",
        &token,
        json!({"registry_id": registry_id, "env": "prod"}),
    )
    .await;
    assert_eq!(status, 403, "{body}");

    let (status, body) = post_auth(
        &srv.base,
        "/api/runs",
        &token,
        json!({"registry_id": registry_id, "env": "staging"}),
    )
    .await;
    assert_eq!(status, 202, "{body}");
}

/// Same scope rule for schedules: out-of-scope `env` is a 403, in-scope is
/// created.
#[tokio::test]
async fn create_schedule_rejects_env_outside_the_principals_scopes() {
    let srv = spawn_server().await;
    let token = add_scoped_token(&srv, "sched-op", "operator", &["staging"]).await;
    let registry_id = register_def(&srv.base, &token).await;

    let (status, body) = post_auth(
        &srv.base,
        "/api/schedules",
        &token,
        json!({"name": "hourly", "registry_id": registry_id, "interval_s": 3600, "env": "prod"}),
    )
    .await;
    assert_eq!(status, 403, "{body}");

    let (status, body) = post_auth(
        &srv.base,
        "/api/schedules",
        &token,
        json!({"name": "hourly", "registry_id": registry_id, "interval_s": 3600, "env": "staging"}),
    )
    .await;
    assert_eq!(status, 201, "{body}");
}

/// Same scope rule for GameDay campaigns: out-of-scope `env` is a 403,
/// in-scope launches.
#[tokio::test]
async fn start_campaign_rejects_env_outside_the_principals_scopes() {
    let srv = spawn_server().await;
    let token = add_scoped_token(&srv, "gd-op", "operator", &["staging"]).await;
    let (status, body) = post_auth(
        &srv.base,
        "/api/gamedays/validate",
        &token,
        json!({"toon": GAMEDAY, "experiments": {"a.toon": SCOPE_TOON}}),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let gameday_id = body["gameday_registry_id"].as_str().unwrap().to_string();

    let (status, body) = post_auth(
        &srv.base,
        &format!("/api/gamedays/{gameday_id}/runs"),
        &token,
        json!({"env": "prod"}),
    )
    .await;
    assert_eq!(status, 403, "{body}");

    let (status, body) = post_auth(
        &srv.base,
        &format!("/api/gamedays/{gameday_id}/runs"),
        &token,
        json!({"env": "staging"}),
    )
    .await;
    assert_eq!(status, 202, "{body}");

    // The 403 above did not launch anything, so exactly one campaign exists.
    let detail: Value = get_auth(&srv.base, "/api/runs?limit=500", &token).await.1;
    assert_eq!(detail["count"], 1, "{detail}");
}

/// The `registry_id` filter of the run list keeps the scope rule: a reader
/// scoped to staging finds the staging campaign of its GameDay and not the
/// prod campaign of another.
#[tokio::test]
async fn the_gameday_filter_of_the_run_list_keeps_the_scope_rule() {
    let srv = spawn_server().await;
    let admin = add_scoped_token(&srv, "gd-admin", "admin", &[]).await;
    let reader = add_scoped_token(&srv, "gd-reader", "viewer", &["staging"]).await;
    let mut campaigns = Vec::new();
    for (title, env) in [("staging campaign", "staging"), ("prod campaign", "prod")] {
        let toon = GAMEDAY.replace("scoped campaign", title);
        let (status, body) = post_auth(
            &srv.base,
            "/api/gamedays/validate",
            &admin,
            json!({"toon": toon, "experiments": {"a.toon": SCOPE_TOON}}),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        let gameday_id = body["gameday_registry_id"].as_str().unwrap().to_string();
        let (status, body) = post_auth(
            &srv.base,
            &format!("/api/gamedays/{gameday_id}/runs"),
            &admin,
            json!({"env": env}),
        )
        .await;
        assert_eq!(status, 202, "{body}");
        campaigns.push((gameday_id, body["run_id"].clone()));
    }
    let [(staging_day, staging_run), (prod_day, prod_run)] = &campaigns[..] else {
        panic!("two campaigns: {campaigns:?}");
    };

    let path = |day: &str| format!("/api/runs?registry_id={day}");
    let (status, body) = get_auth(&srv.base, &path(staging_day), &reader).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["count"], 1, "{body}");
    assert_eq!(&body["runs"][0]["id"], staging_run, "{body}");
    // The scoped read sends the parameters in the shape of the open one.
    assert_eq!(
        body["runs"][0]["params_json"],
        json!({"env": "staging"}),
        "{body}"
    );

    let (status, body) = get_auth(&srv.base, &path(prod_day), &reader).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["count"], 0, "out of scope: {body}");

    let (status, body) = get_auth(&srv.base, &path(prod_day), &admin).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["count"], 1, "{body}");
    assert_eq!(&body["runs"][0]["id"], prod_run, "{body}");
}

/// Preview data must never carry server-resolved credentials, even for an operator.
#[tokio::test]
async fn preview_redacts_resolved_secrets() {
    let srv = spawn_server().await;
    let token = add_scoped_token(&srv, "preview-op", "operator", &[]).await;
    let secret = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(secret.path(), "synthetic-preview-secret-123").unwrap();
    let toon = format!("title: secret preview\nsecrets:\n  demo:\n    token:\n      type: file\n      path: {}\nmethod[1]:\n  - name: preview\n    activity_type: probe\n    provider:\n      type: process\n      path: echo\n      arguments[1]: \"${{secrets.demo.token}}\"\n", secret.path().display());
    let (status, registered) = post_auth(
        &srv.base,
        "/api/runs/validate",
        &token,
        json!({"toon": toon}),
    )
    .await;
    assert_eq!(status, 200, "{registered}");
    assert_eq!(registered["valid"], true, "{registered}");
    let (status, preview) = post_auth(
        &srv.base,
        "/api/runs/dry-run",
        &token,
        json!({"registry_id": registered["registry_id"]}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(preview["valid"], true, "{preview}");
    assert!(
        !preview.to_string().contains("synthetic-preview-secret-123"),
        "preview exposed a secret"
    );
    assert!(preview.to_string().contains("[REDACTED]"));
    let (resolved, injected) = tumult_ingest::prepare_run(&toon, &Default::default()).unwrap();
    let private_hash =
        tumult_ingest::execution_policy::execution_hash(&resolved, &injected).unwrap();
    assert_ne!(
        preview["execution_hash"], private_hash,
        "preview must not be an offline secret verifier"
    );
    assert!(
        preview.get("binding_hash").is_none(),
        "only unrestricted administrators may see the binding digest"
    );
    let admin = add_scoped_token(&srv, "preview-admin", "admin", &[]).await;
    let (_, admin_preview) = post_auth(
        &srv.base,
        "/api/runs/dry-run",
        &admin,
        json!({"registry_id":registered["registry_id"]}),
    )
    .await;
    assert_eq!(admin_preview["binding_hash"], private_hash);
    assert_ne!(admin_preview["execution_hash"], private_hash);
    let scoped_admin = add_scoped_token(&srv, "preview-scoped-admin", "admin", &["staging"]).await;
    let (_, scoped_preview) = post_auth(
        &srv.base,
        "/api/runs/dry-run",
        &scoped_admin,
        json!({"registry_id":registered["registry_id"],"env":"staging"}),
    )
    .await;
    assert!(scoped_preview.get("binding_hash").is_none());
}

fn evidence(env: &str) -> Value {
    json!({"experiment_name":"scope evidence", "exercise_type":"gameday", "executed_at_ns": now_ns(), "hypothesis":"healthy", "method":"manual test", "outcome_status":"passed", "target_environment":env, "attestation":"I attest this test happened"})
}

/// Manual evidence named `name` in `env`.
fn named_evidence(name: &str, env: &str) -> Value {
    let mut record = evidence(env);
    record["experiment_name"] = json!(name);
    record
}

#[tokio::test]
async fn a_name_filter_keeps_manual_records_inside_the_scope() {
    let srv = spawn_server().await;
    let admin = add_scoped_token(&srv, "name-admin", "admin", &[]).await;
    let scoped = add_scoped_token(&srv, "name-scoped", "approver", &["staging"]).await;
    // The name carries both quotes: it is a value, never SQL.
    let name = r#"it's a "drill""#;
    for env in ["prod", "staging"] {
        let record = named_evidence(name, env);
        let (status, made) = post_auth(&srv.base, "/api/manual/experiments", &admin, record).await;
        assert_eq!(status, 201, "{made}");
    }
    let path = "/api/experiments?name=it%27s%20a%20%22drill%22";

    let (status, all) = get_auth(&srv.base, path, &admin).await;
    assert_eq!(status, 200, "{all}");
    let rows = all["experiments"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "both records carry the name: {all}");
    assert!(rows.iter().all(|row| row["name"] == name));

    let (status, own) = get_auth(&srv.base, path, &scoped).await;
    assert_eq!(status, 200, "{own}");
    let rows = own["experiments"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "production evidence leaked by name: {own}");
    assert_eq!(rows[0]["target_environment"], "staging");
    assert_eq!(rows[0]["origin"], "manual");

    // A quote that closes nothing: no row and no error.
    let (status, none) = get_auth(
        &srv.base,
        "/api/experiments?name=%27%20OR%20%271%27=%271",
        &admin,
    )
    .await;
    assert_eq!(status, 200, "{none}");
    assert_eq!(none["experiments"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn manual_scopes_cover_lists_and_every_mutation() {
    let srv = spawn_server().await;
    let admin = add_scoped_token(&srv, "manual-admin", "admin", &[]).await;
    let scoped = add_scoped_token(&srv, "manual-scoped", "approver", &["staging"]).await;
    let (_, made) = post_auth(
        &srv.base,
        "/api/manual/experiments",
        &admin,
        evidence("prod"),
    )
    .await;
    let id = made["id"].as_str().unwrap();
    let (status, rows) = get_auth(&srv.base, "/api/manual/experiments", &scoped).await;
    assert_eq!(status, 200);
    assert_eq!(
        rows["records"].as_array().unwrap().len(),
        0,
        "production evidence leaked"
    );
    for (suffix, body) in [
        ("submit", json!({})),
        ("verify", json!({})),
        ("reject", json!({"note":"no"})),
        (
            "attachments",
            json!({"kind":"url","uri":"https://example.com"}),
        ),
    ] {
        let (status, body) = post_auth(
            &srv.base,
            &format!("/api/manual/experiments/{id}/{suffix}"),
            &scoped,
            body,
        )
        .await;
        assert_eq!(status, 404, "{suffix}: {body}");
    }
    let resp = reqwest::Client::new()
        .put(format!("{}/api/manual/experiments/{id}", srv.base))
        .bearer_auth(&scoped)
        .json(&evidence("staging"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
    let (status, _) = post_auth(
        &srv.base,
        "/api/manual/experiments",
        &scoped,
        evidence("prod"),
    )
    .await;
    assert_eq!(status, 403);
    let (status, _) = post_auth(
        &srv.base,
        "/api/manual/import",
        &scoped,
        json!({"records":[evidence("prod")]}),
    )
    .await;
    assert_eq!(status, 403);
    let (status, made) = post_auth(
        &srv.base,
        "/api/manual/experiments",
        &scoped,
        evidence("staging"),
    )
    .await;
    assert_eq!(status, 201, "{made}");
    let id = made["id"].as_str().unwrap();
    let (status, _) = post_auth(
        &srv.base,
        &format!("/api/manual/experiments/{id}/submit"),
        &scoped,
        json!({}),
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn pending_runs_and_schedules_remain_scoped_before_telemetry_exists() {
    let srv = spawn_server().await;
    let admin = add_scoped_token(&srv, "pending-admin", "admin", &[]).await;
    let scoped = add_scoped_token(&srv, "pending-scoped", "approver", &["staging"]).await;
    let registry_id = register_def(&srv.base, &admin).await;
    let (_, made) = post_auth(
        &srv.base,
        "/api/runs",
        &admin,
        json!({"registry_id":registry_id,"env":"prod"}),
    )
    .await;
    let id = made["run_id"].as_str().unwrap();
    for path in [
        format!("/api/runs/{id}"),
        format!("/api/runs/{id}/audit/verify"),
    ] {
        assert_eq!(get_auth(&srv.base, &path, &scoped).await.0, 404, "{path}");
    }
    for suffix in ["stop", "approve", "reject"] {
        assert_eq!(
            post_auth(
                &srv.base,
                &format!("/api/runs/{id}/{suffix}"),
                &scoped,
                json!({})
            )
            .await
            .0,
            404,
            "{suffix}"
        );
    }
    for (path, key) in [
        ("/api/runs", "runs"),
        ("/api/approvals", "queue"),
        ("/api/events", "events"),
    ] {
        let (_, rows) = get_auth(&srv.base, path, &scoped).await;
        assert!(rows[key].as_array().unwrap().is_empty(), "{path}: {rows}");
    }
    assert_eq!(
        post_auth(&srv.base, "/api/runs/stop-all", &scoped, json!({}))
            .await
            .1["requested"],
        0
    );
    let (_, schedule) = post_auth(
        &srv.base,
        "/api/schedules",
        &admin,
        json!({"name":"prod","registry_id":registry_id,"env":"prod","interval_s":3600}),
    )
    .await;
    let sid = schedule["id"].as_str().unwrap();
    assert!(
        get_auth(&srv.base, "/api/schedules", &scoped).await.1["schedules"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    for suffix in ["enable", "delete"] {
        assert_eq!(
            post_auth(
                &srv.base,
                &format!("/api/schedules/{sid}/{suffix}"),
                &scoped,
                json!({"enabled":false})
            )
            .await
            .0,
            404
        );
    }
}

#[tokio::test]
async fn scoped_admin_cannot_escape_through_global_operations() {
    let srv = spawn_server().await;
    let token = add_scoped_token(&srv, "scoped-admin", "admin", &["staging"]).await;
    for path in [
        "/api/users",
        "/api/tokens",
        "/api/webhooks",
        "/api/lake/status",
    ] {
        assert_eq!(get_auth(&srv.base, path, &token).await.0, 403, "{path}");
    }
    for path in [
        "/api/users",
        "/api/tokens",
        "/api/webhooks",
        "/api/lake/export",
        "/api/import/journal",
    ] {
        assert_eq!(
            post_auth(&srv.base, path, &token, json!({})).await.0,
            403,
            "{path}"
        );
    }
}

#[tokio::test]
async fn unbound_faults_cannot_use_a_dev_label_to_reduce_risk() {
    let srv = spawn_server().await;
    let admin = add_scoped_token(&srv, "unbound-admin", "admin", &[]).await;
    let scoped = add_scoped_token(&srv, "unbound-scoped", "operator", &["dev"]).await;
    let toon = SCOPE_TOON.replace("scoped launch test experiment", "unreviewed definition");
    let (_, registered) = post_auth(
        &srv.base,
        "/api/runs/validate",
        &admin,
        json!({"toon":toon}),
    )
    .await;
    let body = json!({"registry_id":registered["registry_id"],"env":"dev"});
    let (status, run) = post_auth(&srv.base, "/api/runs", &admin, body.clone()).await;
    assert_eq!(status, 202, "{run}");
    assert_eq!(run["tier"], "T3");
    let (status, _) = post_auth(&srv.base, "/api/runs", &scoped, body.clone()).await;
    assert_eq!(status, 403);
    let (status, preview) = post_auth(&srv.base, "/api/runs/dry-run", &scoped, body).await;
    assert_eq!(status, 200);
    assert_eq!(preview["execution_hash"].as_str().unwrap().len(), 64);
    assert!(preview["execution_context"]["error"].is_string());
}

#[tokio::test]
async fn start_rejects_a_stale_preview_fingerprint() {
    let srv = spawn_server().await;
    let operator = add_scoped_token(&srv, "stale-preview", "operator", &[]).await;
    let registry_id = register_def(&srv.base, &operator).await;
    let (status, body) = post_auth(
        &srv.base,
        "/api/runs",
        &operator,
        json!({"registry_id":registry_id,"env":"dev","execution_hash":"stale"}),
    )
    .await;
    assert_eq!(status, 409, "{body}");
    assert_eq!(
        get_auth(&srv.base, "/api/runs", &operator).await.1["count"],
        0
    );
}

#[tokio::test]
async fn evidence_editor_cannot_review_their_own_changes() {
    let srv = spawn_server().await;
    let author = add_scoped_token(&srv, "evidence-author", "operator", &[]).await;
    let editor = add_scoped_token(&srv, "evidence-editor", "approver", &[]).await;
    let reviewer = add_scoped_token(&srv, "independent-reviewer", "approver", &[]).await;
    let (_, record) = post_auth(
        &srv.base,
        "/api/manual/experiments",
        &author,
        evidence("staging"),
    )
    .await;
    let id = record["id"].as_str().unwrap();
    let response = reqwest::Client::new()
        .put(format!("{}/api/manual/experiments/{id}", srv.base))
        .bearer_auth(&editor)
        .json(&evidence("staging"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        post_auth(
            &srv.base,
            &format!("/api/manual/experiments/{id}/submit"),
            &editor,
            json!({})
        )
        .await
        .0,
        200
    );
    assert_eq!(
        post_auth(
            &srv.base,
            &format!("/api/manual/experiments/{id}/verify"),
            &editor,
            json!({})
        )
        .await
        .0,
        400
    );
    assert_eq!(
        post_auth(
            &srv.base,
            &format!("/api/manual/experiments/{id}/verify"),
            &reviewer,
            json!({})
        )
        .await
        .0,
        200
    );
}
