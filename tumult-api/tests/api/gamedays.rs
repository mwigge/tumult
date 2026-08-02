use crate::common::*;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// /api/gamedays* — GameDay registration and inspection (campaign execution
// lands separately)

const EXP_A: &str = "
title: campaign step A
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

const EXP_B: &str = "
title: campaign step B
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
title: smoke campaign
description: two-step drill
tags[1]: demo

regulatory:
  frameworks[1]: DORA
  requirements[1]:
    - id: Art. 25
      description: scenario testing
      evidence: quarterly gameday

experiments[2]:
  - path: a.toon
    compliance_maps[1]: Art. 25
  - path: b.toon
    compliance_maps[0]:

scoring:
  pass_threshold: 0.75
  mttr_target_s: 30.0
  recovery_required: true
";

async fn validate_gameday(base: &str, toon: &str, experiments: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/gamedays/validate"))
        .json(&json!({"toon": toon, "experiments": experiments}))
        .send()
        .await
        .unwrap();
    (resp.status().as_u16(), resp.json().await.unwrap())
}

fn experiment_map() -> Value {
    json!({"a.toon": EXP_A, "b.toon": EXP_B})
}

#[tokio::test]
async fn validate_registers_gameday_and_experiments() {
    let srv = spawn_server().await;

    let (status, body) = validate_gameday(&srv.base, GAMEDAY, experiment_map()).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["valid"], true, "{body}");
    let gameday_id = body["gameday_registry_id"].as_str().unwrap().to_string();
    let experiments = body["experiments"].as_array().unwrap();
    assert_eq!(experiments.len(), 2);
    assert_eq!(experiments[0]["path"], "a.toon");
    assert_eq!(experiments[1]["path"], "b.toon");
    let reg_a = experiments[0]["registry_id"].as_str().unwrap().to_string();

    // The gameday and its experiments share the registry; re-validating
    // dedups onto the same ids.
    let (status, body) = validate_gameday(&srv.base, GAMEDAY, experiment_map()).await;
    assert_eq!(status, 200);
    assert_eq!(body["gameday_registry_id"], gameday_id);
    assert_eq!(body["experiments"][0]["registry_id"], reg_a);

    // The experiments are ordinary runnable definitions.
    let resp = reqwest::Client::new()
        .get(format!("{}/api/registry/{reg_a}", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["definition"]["name"], "campaign step A");

    // The gameday appears in the list, the experiment definitions do not
    // crowd it.
    let resp = reqwest::Client::new()
        .get(format!("{}/api/gamedays", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    let days = body["gamedays"].as_array().unwrap();
    assert_eq!(days.len(), 1, "{body}");
    assert_eq!(days[0]["id"], gameday_id);
    assert_eq!(days[0]["name"], "smoke campaign");

    // The detail returns the parsed campaign plan in order.
    let resp = reqwest::Client::new()
        .get(format!("{}/api/gamedays/{gameday_id}", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["title"], "smoke campaign");
    assert_eq!(body["description"], "two-step drill");
    assert_eq!(body["tags"], json!(["demo"]));
    assert_eq!(body["scoring"]["pass_threshold"], 0.75);
    assert_eq!(body["regulatory"]["frameworks"], json!(["DORA"]));
    let steps = body["experiments"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["name"], "campaign step A");
    assert_eq!(steps[0]["registry_id"], reg_a);
    assert_eq!(steps[0]["compliance_maps"], json!(["Art. 25"]));
    assert_eq!(steps[1]["name"], "campaign step B");

    // Unknown id: 404.
    let resp = reqwest::Client::new()
        .get(format!("{}/api/gamedays/reg-nope", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn validate_rejects_bad_input() {
    let srv = spawn_server().await;

    // Experiment path referenced but not supplied.
    let (status, body) = validate_gameday(&srv.base, GAMEDAY, json!({"a.toon": EXP_A})).await;
    assert_eq!(status, 400, "{body}");

    // The gameday itself does not parse.
    let (status, body) =
        validate_gameday(&srv.base, "title: nope\nunknown_field: 1", experiment_map()).await;
    assert_eq!(status, 400, "{body}");

    // An experiment that fails the run pipeline.
    let (status, body) = validate_gameday(
        &srv.base,
        GAMEDAY,
        json!({"a.toon": EXP_A, "b.toon": "title: broken\nmethod[0]:"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
}

#[tokio::test]
async fn start_campaign_creates_the_parent_run() {
    let srv = spawn_server().await;
    let (status, body) = validate_gameday(&srv.base, GAMEDAY, experiment_map()).await;
    assert_eq!(status, 200, "{body}");
    let gameday_id = body["gameday_registry_id"].as_str().unwrap().to_string();

    // Unknown gameday: 404.
    let resp = reqwest::Client::new()
        .post(format!("{}/api/gamedays/reg-nope/runs", srv.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);

    // Launch: 202 with the parent run id and the step count.
    let resp = reqwest::Client::new()
        .post(format!("{}/api/gamedays/{gameday_id}/runs", srv.base))
        .json(&json!({"env": "dev"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 202);
    let body: Value = resp.json().await.unwrap();
    let run_id = body["run_id"].as_str().unwrap().to_string();
    assert_eq!(body["state"], "queued");
    assert_eq!(body["steps"], 2);

    // A second campaign for the same gameday conflicts while the first is
    // active.
    let resp = reqwest::Client::new()
        .post(format!("{}/api/gamedays/{gameday_id}/runs", srv.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 409);

    // The parent is an ordinary run row; its (empty) child list is
    // filterable by gameday_id.
    let resp = reqwest::Client::new()
        .get(format!("{}/api/runs/{run_id}", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let resp = reqwest::Client::new()
        .get(format!("{}/api/runs?gameday_id={run_id}", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["count"], 0, "no children yet: {body}");
}
