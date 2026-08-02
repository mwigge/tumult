use crate::common::*;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// /api/events — cross-run audit feed over run_audit (newest first)

const DEF_TOON: &str = "
title: events feed test experiment
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

/// Create a gated run (its request/decision events seed the feed).
async fn seed_run(base: &str) -> String {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/runs/validate"))
        .json(&json!({"toon": DEF_TOON}))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    let registry_id = body["registry_id"].as_str().unwrap();
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/runs"))
        .json(&json!({"registry_id": registry_id}))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    body["run_id"].as_str().unwrap().to_string()
}

async fn get_events(base: &str, qs: &str) -> Value {
    let resp = reqwest::Client::new()
        .get(format!("{base}/api/events{qs}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    resp.json().await.unwrap()
}

#[tokio::test]
async fn events_feed_newest_first_with_hash_chain() {
    let srv = spawn_server().await;
    let run_id = seed_run(&srv.base).await;
    // Stop it: adds a stop_requested event after the requested event.
    let resp = reqwest::Client::new()
        .post(format!("{}/api/runs/{run_id}/stop", srv.base))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);

    let body = get_events(&srv.base, "?limit=10").await;
    let events = body["events"].as_array().unwrap();
    assert!(events.len() >= 2, "{body}");
    // Newest first: aborted (terminal) precedes the stop request, which
    // precedes the run's approval request.
    let names: Vec<&str> = events.iter().filter_map(|e| e["event"].as_str()).collect();
    assert_eq!(names, ["aborted", "stop_requested", "requested"], "{body}");
    // Every row carries its hash-chain links and the joined definition name.
    for e in events {
        assert!(e["new_hash"].as_str().is_some(), "{e}");
        assert!(e.get("prev_hash").is_some(), "{e}");
        assert_eq!(e["definition_name"], "events feed test experiment");
    }
    let timestamps: Vec<i64> = events
        .iter()
        .map(|e| e["at_ns"].as_i64().unwrap())
        .collect();
    let mut sorted = timestamps.clone();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    assert_eq!(timestamps, sorted, "newest first");

    // Filter by run: only this run's events.
    let body = get_events(&srv.base, &format!("?run_id={run_id}")).await;
    let filtered = body["events"].as_array().unwrap();
    assert!(!filtered.is_empty());
    assert!(filtered.iter().all(|e| e["run_id"] == run_id));

    // Filter by event type.
    let body = get_events(&srv.base, "?event=stop_requested").await;
    let filtered = body["events"].as_array().unwrap();
    assert!(!filtered.is_empty());
    assert!(filtered.iter().all(|e| e["event"] == "stop_requested"));

    // Limit is honored; the cursor pages backwards.
    let page1 = get_events(&srv.base, "?limit=1").await;
    let first = &page1["events"][0];
    let page2 = get_events(&srv.base, &format!("?limit=1&before={}", first["at_ns"])).await;
    let second = &page2["events"][0];
    assert!(second["at_ns"].as_i64().unwrap() < first["at_ns"].as_i64().unwrap());

    // Limit caps at 200.
    let body = get_events(&srv.base, "?limit=9999").await;
    assert!(body["events"].as_array().unwrap().len() <= 200);
}
