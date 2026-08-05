use crate::common::*;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// RBAC roles and env scopes on the mutation endpoints: dry-run is gated at
// Operator (the resolved plan carries substituted secrets), and run /
// schedule / gameday launches reject an `env` outside the principal's
// scopes (the same rule the reads already apply).

const SCOPE_TOON: &str = "
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

/// `POST /api/runs/dry-run` returns the fully resolved plan — including
/// substituted `${secrets.*}` — so a Viewer is refused (403) while an
/// Operator gets the plan.
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
