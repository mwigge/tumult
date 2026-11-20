//! `GET /api/gate/{target}` — the CI gate: one target's compliance score
//! turned into a pass/warn/fail verdict with the evidence behind it.

use crate::common::*;

/// Seed: `database` has pg-failover (passed, 100) and cache-stampede
/// (failed, 50) → score 75, band good. A failed experiment keeps the verdict
/// at `warn` even though the score clears the default floor.
#[tokio::test]
async fn gate_returns_verdict_with_evidence() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/gate/database").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["target"], "database");
    assert_eq!(body["score"], 75.0);
    assert_eq!(body["band"], "good");
    assert_eq!(body["verdict"], "warn", "{body}");
    assert_eq!(body["thresholds"]["min_score"], 70.0);
    assert_eq!(body["thresholds"]["max_age_days"], 30);
    let reasons = body["reasons"].as_array().unwrap();
    assert!(
        reasons
            .iter()
            .any(|r| r.as_str().unwrap().contains("cache-stampede")),
        "{body}"
    );
    let evidence = &body["evidence"];
    assert_eq!(evidence["runs"], 2);
    assert_eq!(evidence["last_run_id"], "exp-fail", "{body}");
    assert!(evidence["last_run_at_ns"].is_number());
    assert_eq!(evidence["experiments"].as_array().unwrap().len(), 2);
    assert!(body["as_of_ns"].is_number());
}

/// Raising the floor above the score fails the gate; lowering it and
/// ignoring the failed experiment is still a warn (failures always surface).
#[tokio::test]
async fn gate_fails_below_min_score() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/gate/database?min_score=80").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "fail");
    assert!(
        body["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap().contains("below")),
        "{body}"
    );
}

/// Evidence older than `max_age_days` is stale: warn, never pass.
#[tokio::test]
async fn gate_warns_on_stale_evidence() {
    let srv = spawn_server().await;
    // Seed runs are 10 and 30 minutes old; a 0-day window makes them stale.
    let (status, body) = get(&srv.base, "/api/gate/database?max_age_days=0").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["verdict"], "warn");
    assert!(
        body["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap().contains("stale")),
        "{body}"
    );
}

/// An unknown target is a 404, not a silent pass.
#[tokio::test]
async fn gate_unknown_target_is_404() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/gate/no-such-target").await;
    assert_eq!(status, 404, "{body}");
    assert!(body["error"].is_string());
}

/// Bad thresholds are rejected up front.
#[tokio::test]
async fn gate_rejects_invalid_thresholds() {
    let srv = spawn_server().await;
    let (status, _) = get(&srv.base, "/api/gate/database?min_score=101").await;
    assert_eq!(status, 400);
    let (status, _) = get(&srv.base, "/api/gate/database?min_score=-1").await;
    assert_eq!(status, 400);
}

/// Scoped principals: an `env` outside the scopes is a 403; without `env`
/// the gate only sees the principal's own environments, so the demo-only
/// seed is invisible to a prod-scoped viewer.
#[tokio::test]
async fn gate_respects_environment_scopes() {
    let srv = spawn_server().await;
    let token = add_scoped_token(&srv, "gate-viewer", "viewer", &["prod"]).await;
    let (status, body) = get_auth(&srv.base, "/api/gate/database?env=demo", &token).await;
    assert_eq!(status, 403, "{body}");
    let (status, body) = get_auth(&srv.base, "/api/gate/database", &token).await;
    assert_eq!(status, 404, "{body}");

    let demo = add_scoped_token(&srv, "gate-demo", "viewer", &["demo"]).await;
    let (status, body) = get_auth(&srv.base, "/api/gate/database", &demo).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["score"], 75.0);
}
