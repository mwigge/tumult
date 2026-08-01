use crate::common::*;
use serde_json::Value;

// ---------------------------------------------------------------------------
// /api/scores + /api/reports/v2/*

#[tokio::test]
async fn scores_returns_freshness_decayed_card() {
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/scores").await;
    assert_eq!(status, 200, "{body}");

    let experiments = body["experiments"].as_array().unwrap();
    assert_eq!(experiments.len(), 2, "{body}");
    let pass = experiments
        .iter()
        .find(|e| e["name"] == "pg-failover")
        .unwrap();
    assert_eq!(pass["score"], 100);
    assert_eq!(pass["state"], "passed");
    assert_eq!(pass["band"], "good");
    let fail = experiments
        .iter()
        .find(|e| e["name"] == "cache-stampede")
        .unwrap();
    assert_eq!(fail["score"], 50);
    assert_eq!(fail["state"], "failed");

    // Both experiments target "database": one target at (100+50)/2.
    let targets = body["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0]["target"], "database");
    assert_eq!(targets[0]["score"], 75.0);
    assert_eq!(body["portfolio"], 75.0);
    assert_eq!(body["band"], "good");
    assert!(body["delta"].is_number(), "delta should compare windows");

    let (status, _) = get(&srv.base, "/api/scores?range=1y").await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn reports_v2_executive_digest_roundtrip() {
    let srv = spawn_server().await;
    let resp = reqwest::Client::new()
        .post(format!("{}/api/reports/v2/generate", srv.base))
        .json(&serde_json::json!({"type": "executive-digest", "period": "7d"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let meta: Value = resp.json().await.unwrap();
    let id = meta["doc_id"].as_str().unwrap();
    assert!(id.starts_with("KRK-R1-"), "{id}");
    assert_eq!(meta["type"], "executive-digest");
    assert_eq!(meta["sha256"].as_str().unwrap().len(), 64);
    assert!(meta["bytes"].as_u64().unwrap() > 10_000, "{meta}");

    // Listed newest first.
    let (_, list) = get(&srv.base, "/api/reports/v2").await;
    let ids: Vec<&str> = list["reports"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["doc_id"].as_str())
        .collect();
    assert!(ids.contains(&id), "{ids:?}");

    // PDF artifact has the magic bytes.
    let resp = reqwest::get(format!("{}/api/reports/v2/{id}/pdf", srv.base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let bytes = resp.bytes().await.unwrap();
    assert!(bytes.starts_with(b"%PDF"), "missing pdf magic");

    // HTML preview carries the document id.
    let resp = reqwest::get(format!("{}/api/reports/v2/{id}/html", srv.base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains(id));
}

#[tokio::test]
async fn reports_v2_game_day_validates_and_roundtrips() {
    let srv = spawn_server().await;
    let client = reqwest::Client::new();
    // Missing experiment_id → 400.
    let resp = client
        .post(format!("{}/api/reports/v2/generate", srv.base))
        .json(&serde_json::json!({"type": "game-day"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    // Unknown experiment_id → 404.
    let resp = client
        .post(format!("{}/api/reports/v2/generate", srv.base))
        .json(&serde_json::json!({"type": "game-day", "experiment_id": "nope"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    // A real run renders.
    let resp = client
        .post(format!("{}/api/reports/v2/generate", srv.base))
        .json(&serde_json::json!({"type": "game-day", "experiment_id": "exp-pass"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let meta: Value = resp.json().await.unwrap();
    let id = meta["doc_id"].as_str().unwrap();
    assert!(id.starts_with("KRK-R3-"), "{id}");
    let resp = reqwest::get(format!("{}/api/reports/v2/{id}/html", srv.base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.text().await.unwrap().contains("pg-failover"));
}

#[tokio::test]
async fn reports_v2_evidence_pack_validates_framework() {
    let srv = spawn_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/reports/v2/generate", srv.base))
        .json(&serde_json::json!({"type": "evidence-pack", "framework": "hipaa"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let resp = client
        .post(format!("{}/api/reports/v2/generate", srv.base))
        .json(&serde_json::json!({"type": "evidence-pack", "framework": "dora"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let meta: Value = resp.json().await.unwrap();
    assert!(meta["doc_id"].as_str().unwrap().starts_with("KRK-R2-"));
    // The mandatory clause-verification footnote is in the HTML.
    let id = meta["doc_id"].as_str().unwrap();
    let resp = reqwest::get(format!("{}/api/reports/v2/{id}/html", srv.base))
        .await
        .unwrap();
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("verified against the licensed framework text"),
        "{html}"
    );
}

#[tokio::test]
async fn reports_v2_rejects_bad_document_ids() {
    let srv = spawn_server().await;
    let resp = reqwest::get(format!("{}/api/reports/v2/evil..id/pdf", srv.base))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let resp = reqwest::get(format!(
        "{}/api/reports/v2/KRK-R1-20200101-000000/pdf",
        srv.base
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn scores_tree_rolls_up_org_hierarchy() {
    let srv = spawn_server().await;
    // Root: data subtree holds pg-failover (critical, 100); (unassigned)
    // holds cache-stampede (50). Root = (3*100 + 1*50) / 4 = 87.5.
    let (status, body) = get(&srv.base, "/api/scores/tree").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["path"], "");
    assert!(
        (body["score"].as_f64().unwrap() - 87.5).abs() < 1e-9,
        "{}",
        body["score"]
    );
    assert_eq!(body["expected"], 2);
    assert_eq!(body["scored"], 2);
    assert_eq!(body["sparkline"].as_array().unwrap().len(), 10);
    let children = body["children"].as_array().unwrap();
    assert_eq!(children.len(), 2);
    // Weakest first: (unassigned) 50 before data 100.
    assert_eq!(children[0]["name"], "(unassigned)");
    assert_eq!(children[0]["score"], 50.0);
    assert_eq!(children[1]["name"], "data");
    assert_eq!(children[1]["score"], 100.0);

    // Drill one level: node=data.
    let (status, child) = get(&srv.base, "/api/scores/tree?node=data").await;
    assert_eq!(status, 200, "{child}");
    assert_eq!(child["score"], 100.0);
    assert_eq!(child["weakest"], "pg-failover");
    assert_eq!(child["children"][0]["name"], "db-team");
    assert_eq!(child["children"][0]["path"], "data/db-team");

    // Unknown node → 400.
    let (status, _) = get(&srv.base, "/api/scores/tree?node=nope").await;
    assert_eq!(status, 400);
    // Bad range → 400.
    let (status, _) = get(&srv.base, "/api/scores/tree?range=99y").await;
    assert_eq!(status, 400);
}
