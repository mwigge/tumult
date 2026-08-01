use crate::common::*;
use serde_json::{json, Value};

/// A valid manual record body (partial outcome, entered by `by`).
fn manual_body(by: &str) -> Value {
    json!({
        "experiment_name": "pg-manual-gameday",
        "exercise_type": "gameday",
        "executed_at_ns": now_ns() - 3600 * NS,
        "hypothesis": "Failover keeps p95 under 800ms",
        "method": "Disabled the primary; observed failover",
        "outcome_status": "partial",
        "hypothesis_met": true,
        "findings": "Failover worked; warm-up took 40s",
        "action_items": ["Pre-warm the secondary"],
        "target_system": "database",
        "target_environment": "production",
        "recovery_time_s": 40.0,
        "duration_s": 3600.0,
        "entered_by": by,
        "attestation": "I attest this record reflects the exercise as executed.",
        "framework_refs": ["DORA Art. 24(7)"]
    })
}

#[tokio::test]
async fn manual_evidence_lifecycle_end_to_end() {
    let srv = spawn_server().await;
    let client = reqwest::Client::new();
    let base = srv.base.clone();

    // Create a draft.
    let resp = client
        .post(format!("{base}/api/manual/experiments"))
        .json(&manual_body("alice"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let id = resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Listed as draft; not yet scored.
    let (status, list) = get(&base, "/api/manual/experiments?status=draft").await;
    assert_eq!(status, 200);
    assert_eq!(list["records"].as_array().unwrap().len(), 1);
    let (_, scores) = get(&base, "/api/scores").await;
    assert!(scores["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["name"] != "pg-manual-gameday"));

    // Edit the draft (full replace).
    let mut edited = manual_body("alice");
    edited["findings"] = json!("updated findings after replay");
    let resp = client
        .put(format!("{base}/api/manual/experiments/{id}"))
        .json(&edited)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Attach evidence (url ok, file kind rejected — no file storage).
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/attachments"))
        .json(&json!({
            "kind": "url",
            "uri": "https://wiki.example.com/gameday",
            "label": "write-up",
            "added_by": "alice"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/attachments"))
        .json(&json!({"kind": "file", "uri": "/etc/passwd", "added_by": "alice"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // Verify before submit → 409.
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/verify"))
        .json(&json!({"reviewer": "bob"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);

    // Submit locks the record; edits are then rejected.
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/submit"))
        .json(&json!({"by": "alice"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let resp = client
        .put(format!("{base}/api/manual/experiments/{id}"))
        .json(&edited)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);

    // Same-user verify → 400 (segregation of duties).
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/verify"))
        .json(&json!({"reviewer": "alice"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // A second user verifies.
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/verify"))
        .json(&json!({"reviewer": "bob", "note": "evidence reviewed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Detail: verified, audit chain intact, one attachment.
    let (status, detail) = get(&base, &format!("/api/manual/experiments/{id}")).await;
    assert_eq!(status, 200);
    assert_eq!(detail["experiment"]["status"], "verified");
    assert_eq!(detail["experiment"]["reviewed_by"], "bob");
    assert_eq!(detail["attachments"].as_array().unwrap().len(), 1);
    let actions: Vec<&str> = detail["audit"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["action"].as_str().unwrap())
        .collect();
    assert_eq!(actions, ["create", "edit", "attach", "submit", "verify"]);
    let audit = detail["audit"].as_array().unwrap();
    for w in audit.windows(2) {
        assert_eq!(w[0]["new_hash"], w[1]["prev_hash"]);
    }

    // Scored as manual partial (75) with origin.
    let (_, scores) = get(&base, "/api/scores").await;
    let manual = scores["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "pg-manual-gameday")
        .expect("manual record in scores")
        .clone();
    assert_eq!(manual["origin"], "manual");
    assert_eq!(manual["score"], 75);
    assert_eq!(manual["state"], "partial");

    // The experiments list unions manual rows; origin filter works.
    let (status, list) = get(&base, "/api/experiments?origin=manual").await;
    assert_eq!(status, 200);
    let rows = list["experiments"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["origin"], "manual");
    assert_eq!(rows[0]["review_status"], "verified");
    let (_, all) = get(&base, "/api/experiments").await;
    assert!(all["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["origin"] == "manual"));

    // The org tree sees the manual record too (it matches pg-*).
    let (_, tree) = get(&base, "/api/scores/tree?node=data/db-team").await;
    assert_eq!(tree["expected"], 2);

    // R1 executive digest renders with the By-domain section.
    let resp = client
        .post(format!("{base}/api/reports/v2/generate"))
        .json(&json!({"type": "executive-digest", "period": "7d"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let meta: Value = resp.json().await.unwrap();
    let id2 = meta["doc_id"].as_str().unwrap();
    let resp = reqwest::get(format!("{base}/api/reports/v2/{id2}/html"))
        .await
        .unwrap();
    let html = resp.text().await.unwrap();
    assert!(html.contains("By domain"), "{html}");
    assert!(html.contains("Evidence mix"), "{html}");
}

#[tokio::test]
async fn manual_reject_flow_and_import() {
    let srv = spawn_server().await;
    let client = reqwest::Client::new();
    let base = srv.base.clone();

    // Reject requires a note and lands in status rejected.
    let resp = client
        .post(format!("{base}/api/manual/experiments"))
        .json(&manual_body("alice"))
        .send()
        .await
        .unwrap();
    let id = resp.json::<Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    client
        .post(format!("{base}/api/manual/experiments/{id}/submit"))
        .json(&json!({"by": "alice"}))
        .send()
        .await
        .unwrap();
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/reject"))
        .json(&json!({"reviewer": "bob", "note": "  "}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/reject"))
        .json(&json!({"reviewer": "bob", "note": "insufficient evidence"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let (_, detail) = get(&base, &format!("/api/manual/experiments/{id}")).await;
    assert_eq!(detail["experiment"]["status"], "rejected");

    // Bulk import lands as drafts under one batch and does not score.
    let mut second = manual_body("dan");
    second["experiment_name"] = json!("vpn-tabletop");
    second["outcome_status"] = json!("passed");
    let resp = client
        .post(format!("{base}/api/manual/import"))
        .json(&json!({
            "label": "q3-backfill",
            "records": [manual_body("carol"), second]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["ids"].as_array().unwrap().len(), 2);
    let (_, drafts) = get(&base, "/api/manual/experiments?status=draft").await;
    assert_eq!(drafts["records"].as_array().unwrap().len(), 2);
    assert!(drafts["records"]
        .as_array()
        .unwrap()
        .iter()
        .all(|r| r["batch_id"] == body["batch_id"]));
    let (_, scores) = get(&base, "/api/scores").await;
    assert!(scores["experiments"]
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["name"] != "vpn-tabletop"));

    // Unknown id → 404; bad enum → 400.
    let (status, _) = get(&base, "/api/manual/experiments/01JNONE").await;
    assert_eq!(status, 404);
    let mut bad = manual_body("alice");
    bad["exercise_type"] = json!("war-game");
    let resp = client
        .post(format!("{base}/api/manual/experiments"))
        .json(&bad)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn lake_status_and_manual_export_trigger() {
    let srv = spawn_server().await;

    // Nothing exported yet.
    let (status, body) = get(&srv.base, "/api/lake/status").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["files"], 0);
    assert_eq!(body["retention_days"], 0);
    assert!(body["last_export_ns"].is_null());

    // Trigger one export pass (retention off: no delete, but the endpoint
    // still reports `deleted`).
    let resp = reqwest::Client::new()
        .post(format!("{}/api/lake/export", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let report: Value = resp.json().await.unwrap();
    let spans = report["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "spans")
        .unwrap()
        .clone();
    assert!(spans["rows"].as_u64().unwrap() >= 4, "{spans}");
    assert!(!spans["files"].as_array().unwrap().is_empty());
    assert!(spans["watermark_ns"].as_i64().unwrap() > 0);
    assert_eq!(report["deleted"], serde_json::json!({}));

    // Status now reports files, bytes and watermarks.
    let (status, body) = get(&srv.base, "/api/lake/status").await;
    assert_eq!(status, 200, "{body}");
    assert!(body["files"].as_u64().unwrap() > 0);
    assert!(body["bytes"].as_u64().unwrap() > 0);
    assert!(body["last_export_ns"].as_i64().unwrap() > 0);
    assert!(body["watermarks"]["spans"].as_i64().unwrap() > 0);

    // Idempotent re-run: no new rows, no new files.
    let resp = reqwest::Client::new()
        .post(format!("{}/api/lake/export", srv.base))
        .send()
        .await
        .unwrap();
    let second: Value = resp.json().await.unwrap();
    assert!(
        second["tables"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["rows"] == 0),
        "{second}"
    );
}
