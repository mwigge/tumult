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
    // Every seeded run is from the last half hour. A week ago nothing was
    // on record, and a first score is not a change.
    assert!(body["delta"].is_null(), "{body}");

    let (status, _) = get(&srv.base, "/api/scores?range=1y").await;
    assert_eq!(status, 400);
}

/// One passed run of `queue-drain` on the target `queue`, ten days ago.
async fn seed_a_pass_from_ten_days_ago(srv: &TestServer) {
    let ts = now_ns() - 10 * 86_400 * NS;
    exec_write(srv, move |writer| {
        writer
            .insert_spans(&[tumult_lake::SpanRow {
                ts_ns: ts,
                trace_id: "trace-exp-old".into(),
                span_id: "span-exp-old-root".into(),
                span_name: "resilience.experiment".into(),
                span_kind: "Internal".into(),
                duration_ns: 5 * NS,
                service_name: "tumult".into(),
                experiment_id: Some("exp-old".into()),
                experiment_name: Some("queue-drain".into()),
                outcome_status: Some("completed".into()),
                target_system: Some("queue".into()),
                target_environment: Some("demo".into()),
                events: "[]".into(),
                ..Default::default()
            }])
            .map_err(|e| e.to_string())
    })
    .await;
}

#[tokio::test]
async fn the_change_of_the_scores_is_against_what_was_on_record_a_period_ago() {
    let srv = spawn_server().await;
    seed_a_pass_from_ten_days_ago(&srv).await;

    let (status, body) = get(&srv.base, "/api/scores").await;
    assert_eq!(status, 200, "{body}");

    // Now: `database` at 75 and `queue` at 100. A week ago: `queue` alone.
    assert_eq!(body["portfolio"], 87.5);
    assert_eq!(body["delta"], -12.5);
}

/// The server on a store that holds nothing; returns its base URL.
async fn spawn_on_an_empty_store() -> (String, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("k.duckdb");
    tumult_lake::Store::open(&db_path).unwrap();
    let metrics_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../metrics")
        .canonicalize()
        .unwrap();
    // An LLM at a closed port: the scores never ask it.
    let llm = std::sync::Arc::new(tumult_intelligence::llm::OpenAiCompatClient::new(
        "http://127.0.0.1:1".into(),
        None,
        "test-model".into(),
    ));
    let state = tumult_api::ApiState::new(
        db_path,
        metrics_dir,
        tmp.path().join("reports"),
        llm,
        tumult_compliance::OrgTree::empty(),
        None,
        None,
        None,
        false,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, tumult_api::router(state))
            .await
            .unwrap();
    });
    (format!("http://{addr}"), tmp)
}

/// A card with nothing on record has no score, no change and no band of
/// a score.
fn assert_no_score(body: &Value) {
    assert!(body["portfolio"].is_null(), "{body}");
    assert!(body["delta"].is_null(), "{body}");
    assert_eq!(body["band"], "unknown", "{body}");
    assert_eq!(body["experiments"], serde_json::json!([]), "{body}");
}

#[tokio::test]
async fn a_store_with_nothing_on_record_has_no_score() {
    let (base, _tmp) = spawn_on_an_empty_store().await;

    let (status, body) = get(&base, "/api/scores").await;

    assert_eq!(status, 200, "{body}");
    assert_no_score(&body);
}

#[tokio::test]
async fn a_reader_whose_scope_holds_no_run_has_no_score() {
    // Every seeded run is in `demo`.
    let srv = spawn_server().await;
    let token = add_scoped_token(&srv, "stg-viewer", "viewer", &["staging"]).await;

    let (status, body) = get_auth(&srv.base, "/api/scores", &token).await;

    assert_eq!(status, 200, "{body}");
    assert_no_score(&body);
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
