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
async fn the_tree_of_a_store_with_nothing_on_record_has_no_score() {
    let (base, _tmp) = spawn_on_an_empty_store().await;

    let (status, body) = get(&base, "/api/scores/tree").await;

    assert_eq!(status, 200, "{body}");
    assert!(body["score"].is_null(), "{body}");
    assert!(body["delta"].is_null(), "{body}");
    assert_eq!(body["band"], "unknown", "{body}");
    assert_eq!(body["sparkline"], serde_json::json!([]), "{body}");
    // The one child, (unassigned), has no score either.
    let child = &body["children"][0];
    assert_eq!(child["name"], "(unassigned)", "{body}");
    assert!(child["score"].is_null(), "{body}");
    assert_eq!(child["band"], "unknown", "{body}");
}

#[tokio::test]
async fn a_first_score_of_the_tree_inside_the_period_is_not_a_change() {
    let srv = spawn_server().await;
    seed_a_pass_from_ten_days_ago(&srv).await;

    let (status, body) = get(&srv.base, "/api/scores/tree?range=14d").await;

    assert_eq!(status, 200, "{body}");
    // pg-failover (critical, 100), cache-stampede (50) and queue-drain
    // (100): (3*100 + 50 + 100) / 5 = 90.
    assert_eq!(body["score"], 90.0, "{body}");
    // Fourteen days ago nothing was on record.
    assert!(body["delta"].is_null(), "{body}");
    // Ten instants 1.4 days apart end now. The third, 9.8 days ago, is the
    // first after the pass of ten days ago; the runs of the last half hour
    // join at the last one.
    let mut sparkline: Vec<Value> = (3..10).map(|i| serde_json::json!([i, 100.0])).collect();
    sparkline.push(serde_json::json!([10, 90.0]));
    assert_eq!(body["sparkline"], Value::Array(sparkline), "{body}");
}

#[tokio::test]
async fn the_change_of_the_tree_is_against_its_score_a_period_ago() {
    let srv = spawn_server().await;
    seed_a_pass_from_ten_days_ago(&srv).await;

    let (status, body) = get(&srv.base, "/api/scores/tree?range=7d").await;

    assert_eq!(status, 200, "{body}");
    // Seven days ago the pass of ten days ago was all there was: 100. Now
    // the runs of the last half hour give 90.
    assert_eq!(body["score"], 90.0, "{body}");
    assert_eq!(body["delta"], -10.0, "{body}");
    // Every instant has a score, so every instant has a point.
    let mut sparkline: Vec<Value> = (1..10).map(|i| serde_json::json!([i, 100.0])).collect();
    sparkline.push(serde_json::json!([10, 90.0]));
    assert_eq!(body["sparkline"], Value::Array(sparkline), "{body}");
}

/// The root span of a run of `db-failover` on the target `payments`, a
/// minute ago: the run `id` in `env` (no environment for `None`) that
/// ended with `status`.
fn payments_run(id: &str, env: Option<&str>, status: &str) -> tumult_lake::SpanRow {
    tumult_lake::SpanRow {
        ts_ns: now_ns() - 60 * NS,
        trace_id: format!("trace-{id}"),
        span_id: format!("span-{id}-root"),
        span_name: "resilience.experiment".into(),
        span_kind: "Internal".into(),
        duration_ns: 5 * NS,
        service_name: "tumult".into(),
        experiment_id: Some(id.into()),
        experiment_name: Some("db-failover".into()),
        outcome_status: Some(status.into()),
        target_system: Some("payments".into()),
        target_environment: env.map(str::to_string),
        events: "[]".into(),
        ..Default::default()
    }
}

/// A manual record of `db-failover` on `payments` in `env`, passed a
/// minute ago, entered by alice.
fn payments_record(env: Option<&str>) -> tumult_lake::NewManualExperiment {
    tumult_lake::NewManualExperiment {
        experiment_name: "db-failover".into(),
        exercise_type: tumult_lake::ExerciseType::Drill,
        executed_at_ns: now_ns() - 60 * NS,
        hypothesis: "the replica takes over".into(),
        method: "stop the primary".into(),
        outcome: tumult_lake::ManualOutcome::Passed,
        hypothesis_met: Some(true),
        findings: None,
        action_items: vec![],
        target_system: Some("payments".into()),
        target_environment: env.map(str::to_string),
        blast_radius: None,
        recovery_time_s: None,
        duration_s: None,
        entered_by: "alice".into(),
        attestation: "attested".into(),
        renewal_due_ns: None,
        framework_refs: vec![],
    }
}

/// `db-failover` on `payments` ran in two environments and ended
/// differently: it passed in `staging` (the run `run-stg`) and deviated in
/// `prod` (the run `run-prd`). The seeded runs are in `demo`. Returns the
/// token of a viewer scoped to `staging`.
async fn seed_one_name_in_staging_and_prod(srv: &TestServer) -> String {
    exec_write(srv, |writer| {
        writer
            .insert_spans(&[
                payments_run("run-stg", Some("staging"), "completed"),
                payments_run("run-prd", Some("prod"), "deviated"),
            ])
            .map_err(|e| e.to_string())
    })
    .await;
    add_scoped_token(srv, "stg-viewer", "viewer", &["staging"]).await
}

/// The rows of a card: name, environment and evidence id.
fn card_rows(body: &Value) -> Vec<(String, Option<String>, String)> {
    let mut rows: Vec<(String, Option<String>, String)> = body["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| {
            (
                e["name"].as_str().unwrap().to_string(),
                e["environment"].as_str().map(str::to_string),
                e["evidence_id"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    rows.sort();
    rows
}

#[tokio::test]
async fn a_staging_reader_gets_only_the_staging_row_of_a_name_that_also_ran_in_prod() {
    let srv = spawn_server().await;
    let token = seed_one_name_in_staging_and_prod(&srv).await;

    let (status, body) = get_auth(&srv.base, "/api/scores", &token).await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(
        card_rows(&body),
        [(
            "db-failover".into(),
            Some("staging".into()),
            "run-stg".into()
        )],
        "{body}"
    );
    let targets = body["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 1, "{body}");
    assert_eq!(targets[0]["target"], "payments", "{body}");
    assert_eq!(targets[0]["score"], 100.0, "{body}");
    assert_eq!(targets[0]["runs"], 1, "{body}");
    assert_eq!(body["portfolio"], 100.0, "{body}");
    assert_eq!(body["band"], "good", "{body}");
    assert!(!body.to_string().contains("run-prd"), "{body}");
}

#[tokio::test]
async fn an_unscoped_reader_gets_the_row_of_every_environment() {
    let srv = spawn_server().await;
    seed_one_name_in_staging_and_prod(&srv).await;
    let token = add_scoped_token(&srv, "all-viewer", "viewer", &[]).await;

    let (status, body) = get_auth(&srv.base, "/api/scores", &token).await;

    assert_eq!(status, 200, "{body}");
    assert_eq!(
        card_rows(&body),
        [
            (
                "cache-stampede".into(),
                Some("demo".into()),
                "exp-fail".into()
            ),
            ("db-failover".into(), Some("prod".into()), "run-prd".into()),
            (
                "db-failover".into(),
                Some("staging".into()),
                "run-stg".into()
            ),
            ("pg-failover".into(), Some("demo".into()), "exp-pass".into()),
        ],
        "{body}"
    );
    // `database` at (100 + 50) / 2 and `payments` at (100 + 50) / 2.
    let scores: Vec<(&str, f64)> = body["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| (t["target"].as_str().unwrap(), t["score"].as_f64().unwrap()))
        .collect();
    assert_eq!(scores, [("database", 75.0), ("payments", 75.0)], "{body}");
    assert_eq!(body["portfolio"], 75.0, "{body}");
}

#[tokio::test]
async fn the_tree_of_a_staging_reader_scores_the_staging_run_alone() {
    let srv = spawn_server().await;
    let token = seed_one_name_in_staging_and_prod(&srv).await;

    let (status, body) = get_auth(&srv.base, "/api/scores/tree", &token).await;

    assert_eq!(status, 200, "{body}");
    // db-failover lands in (unassigned): the staging pass is its one leaf.
    assert_eq!(body["score"], 100.0, "{body}");
    assert_eq!(body["scored"], 1, "{body}");
    assert_eq!(body["expected"], 1, "{body}");
    assert_eq!(body["weakest"], "db-failover", "{body}");
}

#[tokio::test]
async fn a_pending_manual_record_in_prod_is_no_leaf_for_a_staging_reader() {
    let srv = spawn_server().await;
    let staging = seed_one_name_in_staging_and_prod(&srv).await;
    let prod = add_scoped_token(&srv, "prd-viewer", "viewer", &["prod"]).await;
    // A draft of the same name waits for review in prod.
    exec_write(&srv, |writer| {
        writer
            .create_manual_draft(&payments_record(Some("prod")))
            .map(drop)
            .map_err(|e| e.to_string())
    })
    .await;

    let (status, body) = get_auth(&srv.base, "/api/scores/tree", &staging).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["expected"], 1, "{body}");
    assert_eq!(body["scored"], 1, "{body}");

    // A prod reader expects it: the prod run and the draft.
    let (status, body) = get_auth(&srv.base, "/api/scores/tree", &prod).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["expected"], 2, "{body}");
    assert_eq!(body["scored"], 1, "{body}");
}

#[tokio::test]
async fn a_run_or_a_manual_record_with_no_environment_is_only_in_the_card_of_an_unscoped_reader() {
    let srv = spawn_server().await;
    let scoped = seed_one_name_in_staging_and_prod(&srv).await;
    let unscoped = add_scoped_token(&srv, "all-viewer", "viewer", &[]).await;
    exec_write(&srv, |writer| {
        writer
            .insert_spans(&[payments_run("run-none", None, "completed")])
            .map_err(|e| e.to_string())?;
        let id = writer
            .create_manual_draft(&payments_record(None))
            .map_err(|e| e.to_string())?;
        writer
            .submit_manual(&id, None, "alice")
            .map_err(|e| e.to_string())?;
        writer
            .verify_manual(&id, "bob", None)
            .map_err(|e| e.to_string())
    })
    .await;
    let without_environment = |body: &Value| -> Vec<String> {
        card_rows(body)
            .into_iter()
            .filter(|(_, env, _)| env.is_none())
            .map(|(_, _, evidence)| evidence)
            .collect()
    };

    let (status, body) = get_auth(&srv.base, "/api/scores", &scoped).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        card_rows(&body),
        [(
            "db-failover".into(),
            Some("staging".into()),
            "run-stg".into()
        )],
        "{body}"
    );

    let (status, body) = get_auth(&srv.base, "/api/scores", &unscoped).await;
    assert_eq!(status, 200, "{body}");
    let found = without_environment(&body);
    assert_eq!(found.len(), 2, "{body}");
    assert!(found.contains(&"run-none".to_string()), "{body}");
}

#[tokio::test]
async fn the_portfolio_of_a_scoped_reader_is_the_score_its_gate_judges() {
    let srv = spawn_server().await;
    let token = seed_one_name_in_staging_and_prod(&srv).await;

    let (status, card) = get_auth(&srv.base, "/api/scores", &token).await;
    assert_eq!(status, 200, "{card}");
    let (status, gate) = get_auth(&srv.base, "/api/gate/payments", &token).await;
    assert_eq!(status, 200, "{gate}");

    // `payments` is the one target in staging: the portfolio is its score.
    assert_eq!(gate["score"], 100.0, "{gate}");
    assert_eq!(card["portfolio"], gate["score"], "{card}");
}

#[tokio::test]
async fn the_change_of_a_scoped_card_is_against_its_own_card_a_period_ago() {
    let srv = spawn_server().await;
    let token = seed_one_name_in_staging_and_prod(&srv).await;
    // Twenty days ago the name deviated in staging and passed in prod.
    let earlier = |id: &str, env: &str, status: &str| tumult_lake::SpanRow {
        ts_ns: now_ns() - 20 * 86_400 * NS,
        ..payments_run(id, Some(env), status)
    };
    let rows = [
        earlier("old-stg", "staging", "deviated"),
        earlier("old-prd", "prod", "completed"),
    ];
    exec_write(&srv, move |writer| {
        writer.insert_spans(&rows).map_err(|e| e.to_string())
    })
    .await;

    let (status, body) = get_auth(&srv.base, "/api/scores?range=7d", &token).await;

    assert_eq!(status, 200, "{body}");
    // Staging alone: 100 now and 50 a week ago. The card of every
    // environment a week ago holds the prod pass too.
    assert_eq!(body["portfolio"], 100.0, "{body}");
    assert_eq!(body["delta"], 50.0, "{body}");
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
    // Every seeded run is from the last half hour: of the ten instants of
    // the week, only the last one has a score.
    assert_eq!(body["sparkline"], serde_json::json!([[10, 87.5]]), "{body}");
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
