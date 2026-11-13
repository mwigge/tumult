//! `GET /api/risks`, `GET /api/posture` and `POST /api/risks/{id}/suppress`
//! — the passive risk scan over the seeded store.

use serde_json::{json, Value};

use crate::common::*;

async fn post(base: &str, path: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body = resp.json::<Value>().await.unwrap_or(Value::Null);
    (status, body)
}

/// Seed: `database` has pg-failover (passed) and cache-stampede (failed),
/// both chaos. The scan flags the failed latest run and the six missing
/// disciplines, ranked highest first, and covers the target in the matrix.
#[tokio::test]
async fn risks_lists_ranked_findings_for_seeded_target() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/risks").await;
    assert_eq!(status, 200, "{body}");
    let risks = body["risks"].as_array().unwrap();
    assert!(!risks.is_empty(), "{body}");
    let ranks: Vec<u64> = risks.iter().map(|r| r["rank"].as_u64().unwrap()).collect();
    let mut sorted = ranks.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(ranks, sorted, "ranked highest first");
    assert!(risks
        .iter()
        .all(|r| r["id"].as_str().unwrap().starts_with("r_")));
    assert!(
        risks.iter().any(|r| r["rule"] == "latest_run_failed"
            && r["target"] == "database"
            && r["detail"].as_str().unwrap().contains("cache-stampede")),
        "{body}"
    );
    assert!(
        risks
            .iter()
            .any(|r| r["rule"] == "discipline_gap" && r["target"] == "database"),
        "{body}"
    );
    assert_eq!(body["summary"]["total"], risks.len());
    assert_eq!(body["summary"]["suppressed"], 0);
    let coverage = &body["coverage"];
    assert!(
        coverage["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["target"] == "database"),
        "{body}"
    );
    assert_eq!(coverage["kinds"].as_array().unwrap().len(), 7);
    assert!(body["as_of_ns"].is_number());
}

#[tokio::test]
async fn posture_returns_score_summary_and_top_risks() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/posture").await;
    assert_eq!(status, 200, "{body}");
    let score = body["posture"]["score"].as_f64().unwrap();
    assert!((0.0..=100.0).contains(&score), "{body}");
    assert!(body["posture"]["band"].is_string());
    assert!(
        body["posture"]["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["target"] == "database"),
        "{body}"
    );
    let top = body["top_risks"].as_array().unwrap();
    assert!(!top.is_empty() && top.len() <= 5, "{body}");
    assert_eq!(
        body["summary"]["total"],
        body["summary"]["total"].as_u64().unwrap()
    );
    assert!(body["coverage"]["targets"].is_array());
    assert!(body["nfrs"].is_array(), "{body}");
}

/// Suppressing a finding moves it from `risks` to `suppressed` with the
/// reason attached; the row comes back hash-chained.
#[tokio::test]
async fn suppress_moves_a_risk_aside() {
    let srv = spawn_server().await;
    let (_, before) = get(&srv.base, "/api/risks").await;
    let id = before["risks"][0]["id"].as_str().unwrap().to_string();
    let total = before["summary"]["total"].as_u64().unwrap();

    let (status, row) = post(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        json!({"reason": "accepted until Q1", "expires_in_days": 30, "actor": "ops"}),
    )
    .await;
    assert_eq!(status, 201, "{row}");
    assert_eq!(row["risk_id"], id);
    assert_eq!(row["actor"], "ops");
    assert!(row["expires_at_ns"].as_i64().unwrap() > row["at_ns"].as_i64().unwrap());
    assert!(row["prev_hash"].is_null());
    assert!(row["new_hash"].is_string());

    let (_, after) = get(&srv.base, "/api/risks").await;
    assert_eq!(after["summary"]["total"], total - 1, "{after}");
    assert_eq!(after["summary"]["suppressed"], 1);
    assert!(after["risks"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["id"] != id));
    let hidden = &after["suppressed"][0];
    assert_eq!(hidden["id"], id);
    assert_eq!(hidden["suppression"]["reason"], "accepted until Q1");

    // A second suppression of the same finding chains onto the first.
    let (status, again) = post(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        json!({"reason": "extended", "actor": "ops"}),
    )
    .await;
    assert_eq!(status, 201, "{again}");
    assert_eq!(again["prev_hash"], row["new_hash"]);
}

#[tokio::test]
async fn suppress_validates_input_and_unknown_ids() {
    let srv = spawn_server().await;
    let (status, body) = post(
        &srv.base,
        "/api/risks/r_nope/suppress",
        json!({"reason": "x", "actor": "ops"}),
    )
    .await;
    assert_eq!(status, 404, "{body}");
    let (_, before) = get(&srv.base, "/api/risks").await;
    let id = before["risks"][0]["id"].as_str().unwrap();
    let (status, body) = post(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        json!({"reason": "  ", "actor": "ops"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        json!({"reason": "x", "expires_in_days": 0, "actor": "ops"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    let (status, body) = post(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        json!({"reason": "x"}),
    )
    .await;
    assert_eq!(status, 400, "actor is required while auth is open: {body}");
}

/// Scoped principals: an `env` outside the scopes is a 403; a viewer may
/// read the scan but not suppress.
#[tokio::test]
async fn risks_respect_scopes_and_roles() {
    let srv = spawn_server().await;
    let viewer = add_scoped_token(&srv, "risk-viewer", "viewer", &["demo"]).await;
    let (status, body) = get_auth(&srv.base, "/api/risks?env=prod", &viewer).await;
    assert_eq!(status, 403, "{body}");
    let (status, body) = get_auth(&srv.base, "/api/posture?env=demo", &viewer).await;
    assert_eq!(status, 200, "{body}");
    let (_, risks) = get_auth(&srv.base, "/api/risks", &viewer).await;
    let id = risks["risks"][0]["id"].as_str().unwrap();
    let (status, body) = post_auth(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        &viewer,
        json!({"reason": "nope"}),
    )
    .await;
    assert_eq!(status, 403, "{body}");
    let operator = add_scoped_token(&srv, "risk-operator", "operator", &["demo"]).await;
    let (status, body) = post_auth(
        &srv.base,
        &format!("/api/risks/{id}/suppress"),
        &operator,
        json!({"reason": "accepted"}),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["actor"], "risk-operator");
}
