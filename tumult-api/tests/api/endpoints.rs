// Imported from kronika. Pedantic lints are scoped to tumult-native
// crates; this file predates the pedantic gate (see crate lib.rs).
#![allow(clippy::pedantic)]

//! Endpoint error paths and request validation: malformed filter parameters
//! are 400s, unknown ids are 404s, gated runs park in `pending_approval`,
//! and `/api/ask` validates the question before any LLM involvement.

use serde_json::{json, Value};

use crate::common::{self, get, RUN_TOON};

async fn post(base: &str, path: &str, body: &Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .json(body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap())
}

#[tokio::test]
async fn experiments_rejects_invalid_filters() {
    let srv = common::spawn_server().await;
    let (status, body) = get(&srv.base, "/api/experiments?outcome=exploded").await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("invalid outcome"));

    let (status, body) = get(&srv.base, "/api/experiments?origin=hybrid").await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("invalid origin"));

    let long_q = "x".repeat(201);
    let (status, body) = get(&srv.base, &format!("/api/experiments?q={long_q}")).await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("q too long"));

    let (status, body) = get(&srv.base, "/api/experiments?range=3h").await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("invalid range"));

    // The manual-only branch of the union renders the seeded manual-free
    // store as an empty list, not an error.
    let (status, body) = get(&srv.base, "/api/experiments?origin=manual").await;
    assert_eq!(status, 200);
    assert_eq!(body["count"], json!(0));
}

#[tokio::test]
async fn ask_validates_the_question_before_touching_the_llm() {
    let srv = common::spawn_server().await;
    let (status, body) = post(&srv.base, "/api/ask", &json!({"question": "   "})).await;
    assert_eq!(status, 400);
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("must not be empty"));

    let long = "x".repeat(1001);
    let (status, body) = post(&srv.base, "/api/ask", &json!({"question": long})).await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("too long"));
}

#[tokio::test]
async fn validate_rejects_unparseable_and_oversized_definitions() {
    let srv = common::spawn_server().await;
    let (status, body) = post(
        &srv.base,
        "/api/runs/validate",
        &json!({"toon": "{{{ not toon"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["valid"], json!(false));
    assert!(body["error"].as_str().unwrap().starts_with("parse:"));

    let huge = "x".repeat(256_001);
    let (status, body) = post(&srv.base, "/api/runs/validate", &json!({"toon": huge})).await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("too large"));
}

#[tokio::test]
async fn registry_and_run_lookups_404_unknown_ids() {
    let srv = common::spawn_server().await;
    let (status, body) = get(&srv.base, "/api/registry/reg-nope").await;
    assert_eq!(status, 404);
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("unknown registry id"));

    let too_long = "x".repeat(101);
    let (status, _) = get(&srv.base, &format!("/api/registry/{too_long}")).await;
    assert_eq!(status, 400);

    let (status, _) = post(
        &srv.base,
        "/api/runs/dry-run",
        &json!({"registry_id": "reg-nope"}),
    )
    .await;
    assert_eq!(status, 404);

    let (status, _) = post(&srv.base, "/api/runs", &json!({"registry_id": "reg-nope"})).await;
    assert_eq!(status, 404);

    let (status, body) = get(&srv.base, "/api/runs/run-nope").await;
    assert_eq!(status, 404);
    assert!(body["error"].as_str().unwrap().contains("unknown run id"));

    let (status, _) = get(&srv.base, &format!("/api/runs/{too_long}")).await;
    assert_eq!(status, 400);

    let (status, _) = get(&srv.base, "/api/runs/run-nope/audit/verify").await;
    assert_eq!(status, 404);

    let (status, body) = get(&srv.base, "/api/runs?state=exploded").await;
    assert_eq!(status, 400);
    assert!(body["error"].as_str().unwrap().contains("invalid state"));
}

#[tokio::test]
async fn create_in_a_staging_env_parks_for_approval() {
    let srv = common::spawn_server().await;
    let (status, body) = post(&srv.base, "/api/runs/validate", &json!({"toon": RUN_TOON})).await;
    assert_eq!(status, 200);
    let registry_id = body["registry_id"].as_str().unwrap().to_string();

    // RUN_TOON has one fault kind with a rollback: a staging env classifies
    // it T2, so the run parks in pending_approval instead of executing.
    let (status, body) = post(
        &srv.base,
        "/api/runs",
        &json!({"registry_id": registry_id, "env": "staging"}),
    )
    .await;
    assert_eq!(status, 202);
    assert_eq!(body["state"], json!("pending_approval"));
    assert_eq!(body["tier"], json!("T2"));
    let run_id = body["run_id"].as_str().unwrap();

    // The parked run shows up in the detail view with its approval request.
    let (status, body) = get(&srv.base, &format!("/api/runs/{run_id}")).await;
    assert_eq!(status, 200);
    assert_eq!(body["run"]["state"], json!("pending_approval"));
    assert_eq!(body["approval"]["request"]["tier"], json!("T2"));

    // …and in the state-filtered list.
    let (status, body) = get(&srv.base, "/api/runs?state=pending_approval").await;
    assert_eq!(status, 200);
    assert_eq!(body["count"], json!(1));
}
