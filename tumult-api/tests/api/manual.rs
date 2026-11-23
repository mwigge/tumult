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
        "framework_refs": ["DORA Art. 24(6)"]
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

    // Attach evidence (url ok; a file kind in JSON is refused: a file is
    // sent as a form).
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

    // Same-user verify → 422 (segregation of duties).
    let resp = client
        .post(format!("{base}/api/manual/experiments/{id}/verify"))
        .json(&json!({"reviewer": "alice"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 422);

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

/// POST `body` to a manual-evidence path and expect `status`.
async fn manual_post(base: &str, path: &str, body: Value, status: u16) -> Value {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/manual/{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), status, "{path}");
    resp.json().await.unwrap()
}

/// A submitted record, ready for review; returns its id.
async fn submitted(base: &str) -> String {
    let created = manual_post(base, "experiments", manual_body("alice"), 201).await;
    let id = created["id"].as_str().unwrap().to_string();
    let by = json!({"by": "alice"});
    manual_post(base, &format!("experiments/{id}/submit"), by, 200).await;
    id
}

#[tokio::test]
async fn manual_record_writes_drop_the_cached_star_maps() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let verified = submitted(&base).await;
    let rejected = submitted(&base).await;
    let cached = starmap_as_of(&base).await;
    assert_eq!(starmap_as_of(&base).await, cached, "served from the cache");

    // A verified record is evidence the map shows.
    let review = json!({"reviewer": "bob", "note": "evidence reviewed"});
    manual_post(
        &base,
        &format!("experiments/{verified}/verify"),
        review,
        200,
    )
    .await;
    let after_verify = starmap_as_of(&base).await;
    assert_ne!(after_verify, cached);

    let review = json!({"reviewer": "bob", "note": "insufficient evidence"});
    manual_post(
        &base,
        &format!("experiments/{rejected}/reject"),
        review,
        200,
    )
    .await;
    let after_reject = starmap_as_of(&base).await;
    assert_ne!(after_reject, after_verify);

    let batch = json!({"label": "backfill", "records": [manual_body("carol")]});
    manual_post(&base, "import", batch, 201).await;
    assert_ne!(starmap_as_of(&base).await, after_reject);

    // A refused write changed nothing and leaves the cache alone.
    let kept = starmap_as_of(&base).await;
    let review = json!({"reviewer": "bob", "note": "again"});
    manual_post(
        &base,
        &format!("experiments/{verified}/verify"),
        review,
        409,
    )
    .await;
    assert_eq!(starmap_as_of(&base).await, kept);
}

/// A restore test with every DR field of schema v22.
fn restore_body(by: &str) -> Value {
    let mut body = manual_body(by);
    body["experiment_name"] = json!("payments-db-restore");
    body["exercise_type"] = json!("restore");
    body["recovery_time_s"] = json!(3000.0);
    body["rto_target_s"] = json!(3600.0);
    body["rpo_target_s"] = json!(300.0);
    body["rpo_measured_s"] = json!(120.0);
    body["data_verified"] = json!(true);
    body["participants"] = json!([
        {"name": "alice", "role": "dr lead"},
        {"name": "carol"}
    ]);
    let start = now_ns() - 3600 * NS;
    body["steps"] = json!([
        {"title": "Stop the primary", "actor": "alice",
         "started_at_ns": start, "ended_at_ns": start + 60 * NS, "result": "done"},
        {"title": "Restore the backup", "actor": "carol", "result": "failed",
         "note": "first restore ran out of disk"}
    ]);
    body["actions"] = json!([
        {"title": "Grow the restore volume", "owner": "carol",
         "due_at_ns": now_ns() + 30 * 86_400 * NS, "ticket": "OPS-12"}
    ]);
    body["links"] = json!([
        {"kind": "function", "linked_id": "cif-0001", "label": "payments"},
        {"kind": "run", "linked_id": "exp-restore-1"}
    ]);
    body
}

#[tokio::test]
async fn a_dr_exercise_keeps_every_field_it_was_sent() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let created = manual_post(&base, "experiments", restore_body("alice"), 201).await;
    let id = created["id"].as_str().unwrap().to_string();

    let (status, detail) = get(&base, &format!("/api/manual/experiments/{id}")).await;
    assert_eq!(status, 200, "{detail}");
    let row = &detail["experiment"];
    assert_eq!(row["exercise_type"], "restore");
    assert_eq!(row["rto_target_s"], 3600.0);
    assert_eq!(row["rpo_target_s"], 300.0);
    assert_eq!(row["rpo_measured_s"], 120.0);
    assert_eq!(row["data_verified"], true);
    assert_eq!(
        row["participants"],
        json!([{"name": "alice", "role": "dr lead"}, {"name": "carol", "role": null}])
    );
    let steps = detail["steps"].as_array().unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0]["ordinal"], 1);
    assert_eq!(steps[1]["result"], "failed");
    assert_eq!(steps[1]["note"], "first restore ran out of disk");
    let actions = detail["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0]["owner"], "carol");
    assert_eq!(actions[0]["ticket"], "OPS-12");
    assert_eq!(actions[0]["status"], "open");
    let links = detail["links"].as_array().unwrap();
    assert_eq!(links[0]["kind"], "function");
    assert_eq!(links[1]["linked_id"], "exp-restore-1");

    // The list carries the DR numbers too.
    let (_, list) = get(&base, "/api/manual/experiments?status=draft").await;
    assert_eq!(list["records"][0]["rpo_measured_s"], 120.0);

    // A bad step result or link kind is refused with the reason.
    for (field, value, needle) in [
        (
            "steps",
            json!([{"title": "t", "result": "ok"}]),
            "unknown step result 'ok'",
        ),
        (
            "links",
            json!([{"kind": "service", "linked_id": "x"}]),
            "unknown link kind 'service'",
        ),
        (
            "rto_target_s",
            json!(-5.0),
            "'rto_target_s' must be a number",
        ),
    ] {
        let mut bad = restore_body("alice");
        bad[field] = value;
        let body = manual_post(&base, "experiments", bad, 400).await;
        assert!(body["error"].as_str().unwrap().contains(needle), "{body}");
    }
}

/// POST  and return the status and the body as text.
async fn post_text(base: &str, path: &str, body: &Value) -> (u16, String) {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/manual/{path}"))
        .json(body)
        .send()
        .await
        .unwrap();
    (resp.status().as_u16(), resp.text().await.unwrap())
}

#[tokio::test]
async fn every_manual_body_refuses_a_field_it_does_not_know() {
    let srv = spawn_server().await;
    let base = srv.base.clone();
    let id = submitted(&base).await;
    let draft = manual_post(&base, "experiments", manual_body("alice"), 201).await;
    let draft = draft["id"].as_str().unwrap().to_string();

    let mut record = manual_body("alice");
    record["rpo_measured"] = json!(720);
    let mut step = restore_body("alice");
    step["steps"][0]["owner"] = json!("alice");
    let mut participant = restore_body("alice");
    participant["participants"][0]["team"] = json!("ops");
    let mut action = restore_body("alice");
    action["actions"][0]["status"] = json!("done");
    let mut link = restore_body("alice");
    link["links"][0]["url"] = json!("https://example.com");
    let mut imported = manual_body("carol");
    imported["participants_list"] = json!(["carol"]);
    let cases = [
        ("experiments".to_string(), record, "rpo_measured"),
        ("experiments".to_string(), step, "owner"),
        ("experiments".to_string(), participant, "team"),
        ("experiments".to_string(), action, "status"),
        ("experiments".to_string(), link, "url"),
        (
            "import".to_string(),
            json!({"label": "q4", "batch": 1, "records": [manual_body("carol")]}),
            "batch",
        ),
        (
            "import".to_string(),
            json!({"records": [imported]}),
            "participants_list",
        ),
        (
            format!("experiments/{draft}/submit"),
            json!({"by": "alice", "signed": true}),
            "signed",
        ),
        (
            format!("experiments/{id}/verify"),
            json!({"reviewer": "bob", "role": "dr owner"}),
            "role",
        ),
        (
            format!("experiments/{id}/reject"),
            json!({"reviewer": "bob", "note": "no", "reason": "x"}),
            "reason",
        ),
        (
            format!("experiments/{draft}/attachments"),
            json!({"kind": "url", "uri": "https://wiki.example/x", "added_by": "alice", "file_hash": "00"}),
            "file_hash",
        ),
    ];
    for (path, body, field) in &cases {
        let (status, text) = post_text(&base, path, body).await;
        assert_eq!(status, 422, "{path} {field}: {text}");
        assert!(
            text.contains(&format!("unknown field `{field}`")),
            "{path}: {text}"
        );
    }

    // Nothing of the refused bodies was stored or changed.
    let (_, all) = get(&base, "/api/manual/experiments").await;
    assert_eq!(all["records"].as_array().unwrap().len(), 2);
    let (_, detail) = get(&base, &format!("/api/manual/experiments/{id}")).await;
    assert_eq!(detail["experiment"]["status"], "submitted");
    let (_, detail) = get(&base, &format!("/api/manual/experiments/{draft}")).await;
    assert_eq!(detail["experiment"]["status"], "draft");
    assert!(detail["attachments"].as_array().unwrap().is_empty());
}
