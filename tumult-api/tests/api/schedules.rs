use crate::common::*;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// /api/schedules* — recurring-run CRUD (schema v10 run_schedules)

const DEF_TOON: &str = "
title: schedule api test experiment
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

/// Register DEF_TOON via the validate endpoint; returns its registry id.
async fn register_def(base: &str) -> String {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/runs/validate"))
        .json(&json!({"toon": DEF_TOON}))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["valid"], true, "{body}");
    body["registry_id"].as_str().unwrap().to_string()
}

async fn create_schedule(base: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/schedules"))
        .json(&body)
        .send()
        .await
        .unwrap();
    (resp.status().as_u16(), resp.json().await.unwrap())
}

async fn list_schedules(base: &str) -> Value {
    let resp = reqwest::Client::new()
        .get(format!("{base}/api/schedules"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    resp.json().await.unwrap()
}

#[tokio::test]
async fn schedule_crud_roundtrip() {
    let srv = spawn_server().await;
    let registry_id = register_def(&srv.base).await;
    let client = reqwest::Client::new();

    let (status, body) = create_schedule(
        &srv.base,
        json!({"name": "hourly drill", "registry_id": registry_id, "interval_s": 3600}),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    let id = body["id"].as_str().unwrap().to_string();
    assert_eq!(body["name"], "hourly drill");
    assert_eq!(body["enabled"], true, "schedules start enabled");
    assert!(body["next_run_at_ns"].as_i64().unwrap() > 0);
    assert_eq!(body["created_by"], Value::Null, "open mode has no actor");

    // The list shows it with the registry name joined.
    let list = list_schedules(&srv.base).await;
    let rows = list["schedules"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], id);
    assert_eq!(rows[0]["definition_name"], "schedule api test experiment");
    assert_eq!(rows[0]["interval_s"], 3600);

    // Disable, then re-enable.
    for (flag, expected) in [(false, false), (true, true)] {
        let resp = client
            .post(format!("{}/api/schedules/{id}/enable", srv.base))
            .json(&json!({"enabled": flag}))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let list = list_schedules(&srv.base).await;
        assert_eq!(list["schedules"][0]["enabled"], expected);
    }

    // Unknown schedule id: 404 on enable and delete.
    let resp = client
        .post(format!("{}/api/schedules/s-nope/enable", srv.base))
        .json(&json!({"enabled": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);

    // Delete removes it; a second delete 404s.
    let resp = client
        .post(format!("{}/api/schedules/{id}/delete", srv.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(list_schedules(&srv.base).await["schedules"], json!([]));
    let resp = client
        .post(format!("{}/api/schedules/{id}/delete", srv.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn create_schedule_validates_input() {
    let srv = spawn_server().await;
    let registry_id = register_def(&srv.base).await;

    // Interval bounds: below 60s and above 30d are rejected.
    for bad in [30, 3_000_000] {
        let (status, body) = create_schedule(
            &srv.base,
            json!({"name": "x", "registry_id": registry_id, "interval_s": bad}),
        )
        .await;
        assert_eq!(status, 400, "interval_s {bad}: {body}");
    }
    // Empty and overlong names are rejected.
    let (status, body) = create_schedule(
        &srv.base,
        json!({"name": "   ", "registry_id": registry_id, "interval_s": 3600}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = create_schedule(
        &srv.base,
        json!({"name": "n".repeat(101), "registry_id": registry_id, "interval_s": 3600}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    // Unknown registry id: 404, house convention.
    let (status, body) = create_schedule(
        &srv.base,
        json!({"name": "x", "registry_id": "reg-nope", "interval_s": 3600}),
    )
    .await;
    assert_eq!(status, 404, "{body}");
}
