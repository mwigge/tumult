use crate::common::*;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// /api/runs* — validate, dry-run, enqueue, e-stop, inspect

/// Register RUN_TOON via the validate endpoint; returns its registry id.
async fn register_run_def(base: &str) -> String {
    register_toon(base, RUN_TOON).await
}

/// Register an experiment TOON via the validate endpoint; returns its
/// registry id.
async fn register_toon(base: &str, toon: &str) -> String {
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/runs/validate"))
        .json(&json!({"toon": toon}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["valid"], true, "{body}");
    body["registry_id"].as_str().unwrap().to_string()
}

/// Poll a run's detail until it reaches a terminal state (10s budget).
async fn await_terminal_run(base: &str, run_id: &str) -> Value {
    const TERMINAL: [&str; 8] = [
        "passed",
        "deviated",
        "failed",
        "aborted",
        "orphaned",
        "rollback_pending",
        "rejected",
        "expired",
    ];
    for _ in 0..200 {
        let resp = reqwest::Client::new()
            .get(format!("{base}/api/runs/{run_id}"))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status().as_u16(), 200);
        let body: Value = resp.json().await.unwrap();
        let state = body["run"]["state"].as_str().unwrap_or_default();
        if TERMINAL.contains(&state) {
            return body;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("run {run_id} never reached a terminal state");
}

/// Create a run from a registered definition and clear its T1 approval,
/// returning the run id once queued. Open auth: the synthetic admin's
/// "anonymous" approver differs from the "synthetic" requester, so the
/// segregation-of-duties check passes (RUN_TOON — one fault kind with a
/// rollback on the default "dev" env — classifies T1 and gates).
async fn enqueue_approved(base: &str, registry_id: &str) -> String {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/api/runs"))
        .json(&json!({"registry_id": registry_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 202);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["state"], "pending_approval", "{body}");
    assert_eq!(body["tier"], "T1", "{body}");
    let run_id = body["run_id"].as_str().unwrap().to_string();
    let resp = client
        .post(format!("{base}/api/runs/{run_id}/approve"))
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["state"], "queued", "{body}");
    run_id
}

#[tokio::test]
async fn validate_registers_and_dedups_definitions() {
    let srv = spawn_server().await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/runs/validate", srv.base))
        .json(&json!({"toon": RUN_TOON}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["valid"], true, "{body}");
    assert_eq!(body["registered"], true, "{body}");
    assert_eq!(body["name"], "api run test experiment");
    let registry_id = body["registry_id"].as_str().unwrap();
    assert!(registry_id.starts_with("reg-"), "{registry_id}");

    // Same TOON again: deduped onto the same registry row.
    let resp = client
        .post(format!("{}/api/runs/validate", srv.base))
        .json(&json!({"toon": RUN_TOON}))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["valid"], true, "{body}");
    assert_eq!(body["registered"], false, "{body}");
    assert_eq!(body["registry_id"], registry_id);

    // Empty method: parses but fails validation — diagnostics, no 5xx.
    let resp = client
        .post(format!("{}/api/runs/validate", srv.base))
        .json(&json!({"toon": "title: no method here"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["valid"], false, "{body}");
    assert!(
        body["error"].as_str().unwrap().starts_with("validate:"),
        "{body}"
    );
}

#[tokio::test]
async fn registry_list_and_detail_roundtrip() {
    let srv = spawn_server().await;
    let registry_id = register_run_def(&srv.base).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(format!("{}/api/registry", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    let defs = body["definitions"].as_array().unwrap();
    let entry = defs
        .iter()
        .find(|d| d["id"] == registry_id)
        .expect("registered definition is listed");
    assert_eq!(entry["name"], "api run test experiment");
    assert!(entry["content_hash"].as_str().unwrap().len() == 64);
    // The list carries metadata only; the TOON comes from the detail.
    assert!(entry.get("definition_toon").is_none(), "{entry}");

    let resp = client
        .get(format!("{}/api/registry/{registry_id}", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["definition"]["id"], registry_id);
    assert_eq!(body["definition"]["definition_toon"], RUN_TOON);

    let resp = client
        .get(format!("{}/api/registry/reg-nope", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn dry_run_returns_resolved_plan() {
    let srv = spawn_server().await;
    let registry_id = register_run_def(&srv.base).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/api/runs/dry-run", srv.base))
        .json(&json!({"registry_id": registry_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["valid"], true, "{body}");
    assert_eq!(body["plan"]["title"], "api run test experiment");
    let method = body["plan"]["method"].as_array().unwrap();
    assert_eq!(method.len(), 3);
    assert_eq!(method[0]["name"], "action-1");
    assert_eq!(body["plan"]["rollbacks"].as_array().unwrap().len(), 1);

    // Unknown registry id: 404.
    let resp = client
        .post(format!("{}/api/runs/dry-run", srv.base))
        .json(&json!({"registry_id": "reg-does-not-exist"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn run_lifecycle_end_to_end() {
    let srv = spawn_server().await;
    let registry_id = register_run_def(&srv.base).await;
    let client = reqwest::Client::new();

    // Unknown registry id: 404, nothing enqueued.
    let resp = client
        .post(format!("{}/api/runs", srv.base))
        .json(&json!({"registry_id": "reg-does-not-exist"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);

    // RUN_TOON (one fault kind with rollback, default "dev" env) classifies
    // T1: the create gates, one approval dispatches.
    let run_id = enqueue_approved(&srv.base, &registry_id).await;

    // The run executes to passed; the audit trail records the transitions.
    let detail = await_terminal_run(&srv.base, &run_id).await;
    assert_eq!(detail["run"]["state"], "passed", "{detail}");
    assert_eq!(detail["run"]["rollback_status"], "not_needed");
    assert!(!detail["run"]["experiment_id"].as_str().unwrap().is_empty());
    let events: Vec<&str> = detail["audit"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["event"].as_str())
        .collect();
    assert!(events.contains(&"requested"), "{events:?}");
    assert!(events.contains(&"passed"), "{events:?}");

    // List filter finds it under passed; a bogus state is a 400.
    let resp = client
        .get(format!("{}/api/runs?state=passed", srv.base))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["count"], 1, "{body}");
    assert_eq!(body["runs"][0]["id"], json!(run_id));
    assert_eq!(
        body["runs"][0]["definition_name"],
        "api run test experiment"
    );

    let resp = client
        .get(format!("{}/api/runs?state=bogus", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 400);

    // Unknown run id: 404.
    let resp = client
        .get(format!("{}/api/runs/run-does-not-exist", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);
}

#[tokio::test]
async fn run_audit_verify_reports_chain_validity() {
    let srv = spawn_server().await;
    let registry_id = register_run_def(&srv.base).await;

    // A run executed to a terminal state has an intact hash chain.
    let run_id = enqueue_approved(&srv.base, &registry_id).await;
    let detail = await_terminal_run(&srv.base, &run_id).await;
    assert_eq!(detail["run"]["state"], "passed", "{detail}");

    let (status, body) = get(&srv.base, &format!("/api/runs/{run_id}/audit/verify")).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["run_id"], json!(run_id));
    assert_eq!(body["chain_valid"], true, "{body}");

    // Unknown run id: 404 (tamper detection itself is covered at the lake
    // layer by `verify_run_audit_chain`'s test).
    let (status, _body) = get(&srv.base, "/api/runs/run-does-not-exist/audit/verify").await;
    assert_eq!(status, 404);
}

/// A probe-only definition classifies T0 (no faults, no rollback), so it
/// enqueues directly — the shape needed to exercise queue backpressure.
const PROBE_ONLY_TOON: &str = r#"
title: probe-only health check
method[2]:
  - name: probe-1
    activity_type: probe
    provider:
      type: native
      plugin: test
      function: noop
  - name: probe-2
    activity_type: probe
    provider:
      type: native
      plugin: test
      function: noop
"#;

#[tokio::test]
async fn run_create_backpressure_returns_429_on_overload() {
    let srv = spawn_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{}/api/runs/validate", srv.base))
        .json(&json!({"toon": PROBE_ONLY_TOON}))
        .send()
        .await
        .unwrap();
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["valid"], true, "{body}");
    let registry_id = body["registry_id"].as_str().unwrap();

    // The harness queue: concurrency 1, depth 4 → capacity 5. Fire the
    // whole burst concurrently so every request lands within the first
    // run's ~400ms lifetime (two 200ms noop probes): at most 5 accepted,
    // the rest rejected 429 — never silently queued. A sequential burst is
    // racy: under suite load it can outlive a run and free a permit.
    let mut handles = Vec::new();
    for _ in 0..8 {
        let client = client.clone();
        let base = srv.base.clone();
        let rid = registry_id.to_string();
        handles.push(tokio::spawn(async move {
            client
                .post(format!("{base}/api/runs"))
                .json(&json!({"registry_id": rid}))
                .send()
                .await
                .unwrap()
        }));
    }
    let mut accepted = 0;
    let mut overloaded = 0;
    for handle in handles {
        let resp = handle.await.unwrap();
        match resp.status().as_u16() {
            202 => accepted += 1,
            429 => overloaded += 1,
            other => panic!("unexpected status {other}"),
        }
    }
    assert!(accepted <= 5, "{accepted} accepted beyond queue capacity");
    assert!(
        overloaded >= 3,
        "only {overloaded} × 429 from a burst of 8 against capacity 5"
    );
}

#[tokio::test]
async fn stop_unknown_terminal_and_running_runs() {
    let srv = spawn_server().await;
    let registry_id = register_run_def(&srv.base).await;
    let client = reqwest::Client::new();
    // Every enqueue gates (T1) and clears through the approval flow.
    let enqueue = || enqueue_approved(&srv.base, &registry_id);

    // Unknown run: 404.
    let resp = client
        .post(format!("{}/api/runs/run-does-not-exist/stop", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 404);

    // A finished run: 409 with its terminal state.
    let done_id = enqueue().await;
    let detail = await_terminal_run(&srv.base, &done_id).await;
    assert_eq!(detail["run"]["state"], "passed");
    let resp = client
        .post(format!("{}/api/runs/{done_id}/stop", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 409);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["state"], "passed");

    // A running run: e-stop cancels mid-method, the runner's rollback path
    // unwinds, terminal state is aborted with the audit trail to prove it.
    // STOP_TOON's `hold-*` steps keep the run in `running` for ~3s, so the
    // stop request lands mid-method even on a loaded CI runner.
    let stop_registry = register_toon(&srv.base, STOP_TOON).await;
    let stop_id = enqueue_approved(&srv.base, &stop_registry).await;
    // Wait for the run to actually start (running, not just queued).
    let mut running = false;
    for _ in 0..400 {
        let resp = client
            .get(format!("{}/api/runs/{stop_id}", srv.base))
            .send()
            .await
            .unwrap();
        let body: Value = resp.json().await.unwrap();
        if body["run"]["state"] == "running" {
            running = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(running, "run {stop_id} never reached `running`");
    let resp = client
        .post(format!("{}/api/runs/{stop_id}/stop", srv.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let detail = await_terminal_run(&srv.base, &stop_id).await;
    assert_eq!(detail["run"]["state"], "aborted", "{detail}");
    assert_eq!(detail["run"]["rollback_status"], "completed");
    let events: Vec<&str> = detail["audit"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["event"].as_str())
        .collect();
    assert!(events.contains(&"stop_requested"), "{events:?}");
}

/// Definition with a declared blast radius, a guard, a fault cap and one
/// targeted action (plus a probe step and a non-target argument) to exercise
/// the dry-run scope summary.
const SCOPE_TOON: &str = r#"
title: scope preview experiment
blast_radius: demo stack only
max_concurrent_faults: 2
guards[1]:
  - name: latency guard
    min_breaches: 2
    probe:
      name: p95
      activity_type: probe
      provider:
        type: process
        path: sh
      tolerance:
        type: range
        from: 0.0
        to: 500.0
method[2]:
  - name: pause db
    activity_type: action
    provider:
      type: native
      plugin: docker
      function: pause
      arguments:
        container: demo-postgres
        duration_s: 30
  - name: watch
    activity_type: probe
    provider:
      type: process
      path: sh
rollbacks[1]:
  - name: unpause db
    activity_type: action
    provider:
      type: native
      plugin: docker
      function: unpause
      arguments:
        container: demo-postgres
"#;

/// The dry-run plan carries a `scope` summary for the blast-radius preview:
/// the declared note, the targeted fault actions (probes excluded; only
/// identifying arguments surface as targets), the guards and the fault cap.
#[tokio::test]
async fn dry_run_scope_summarizes_targets_and_guards() {
    let srv = spawn_server().await;
    let registry_id = register_toon(&srv.base, SCOPE_TOON).await;

    let resp = reqwest::Client::new()
        .post(format!("{}/api/runs/dry-run", srv.base))
        .json(&json!({"registry_id": registry_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["valid"], true, "{body}");

    let scope = &body["plan"]["scope"];
    assert_eq!(scope["blast_radius"], "demo stack only", "{body}");
    assert_eq!(scope["max_concurrent_faults"], 2, "{body}");

    let actions = scope["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 1, "probe steps are excluded: {body}");
    assert_eq!(actions[0]["step"], "pause db");
    assert_eq!(actions[0]["provider"], "docker");
    assert_eq!(actions[0]["action"], "pause");
    // Only identifying arguments surface as targets — `duration_s` does not.
    assert_eq!(actions[0]["targets"], json!({"container": "demo-postgres"}));

    let guards = scope["guards"].as_array().unwrap();
    assert_eq!(guards.len(), 1);
    assert_eq!(guards[0]["name"], "latency guard");
    assert_eq!(guards[0]["probe"], "p95");
    assert_eq!(guards[0]["min_breaches"], 2);
}

/// Definitions without a declared blast radius, guards or fault cap still
/// get a scope block — with nulls and empty lists, not a missing field.
#[tokio::test]
async fn dry_run_scope_defaults_when_nothing_declared() {
    let srv = spawn_server().await;
    let registry_id = register_run_def(&srv.base).await;

    let resp = reqwest::Client::new()
        .post(format!("{}/api/runs/dry-run", srv.base))
        .json(&json!({"registry_id": registry_id}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["valid"], true, "{body}");

    let scope = &body["plan"]["scope"];
    assert!(scope.is_object(), "scope is always present: {body}");
    assert_eq!(scope["blast_radius"], Value::Null, "{body}");
    assert_eq!(scope["max_concurrent_faults"], Value::Null, "{body}");
    assert_eq!(scope["guards"], json!([]), "{body}");
    // RUN_TOON's three native noops are actions with no targets.
    let actions = scope["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 3, "{body}");
    assert_eq!(actions[0]["provider"], "test");
    assert_eq!(actions[0]["action"], "noop");
    assert_eq!(actions[0]["targets"], json!({}));
}
