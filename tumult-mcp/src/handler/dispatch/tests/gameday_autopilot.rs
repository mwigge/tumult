//! Handler round-trips for the `GameDay` and autopilot dispatch bodies that
//! the conformance suite does not reach: enact-lock interplay, the
//! `execute=true`/`approve=true` ledger branches, and `autopilot_notify`.

use super::*;

/// Seed a workspace with one runnable experiment and a gameday campaign that
/// references it (both created through the tool surface).
async fn create_runnable_gameday(handler: &TumultHandler, name: &str) {
    let params = call_params(
        "tumult_gameday_create",
        serde_json::json!({ "name": name, "experiments": ["test.toon"] }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("create must produce a result");
    assert!(
        result.is_error.is_none(),
        "gameday create failed: {}",
        result_text(&result)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_gameday_run_then_analyze_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());
    create_runnable_gameday(&handler, "rt-gd").await;

    let params = call_params(
        "tumult_gameday_run",
        serde_json::json!({ "gameday_path": "rt-gd.gameday.toon" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("run must produce a result");
    assert!(result.is_error.is_none(), "{}", result_text(&result));
    let text = result_text(&result);
    assert!(text.contains("GameDay: rt-gd"), "{text}");
    assert!(text.contains("Experiments: 1/1 passed"), "{text}");
    assert!(
        tmp.path().join("rt-gd.gameday.journal.toon").exists(),
        "the campaign journal must be written"
    );

    let params = call_params(
        "tumult_gameday_analyze",
        serde_json::json!({ "gameday_path": "rt-gd.gameday.toon" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("analyze must produce a result");
    assert!(result.is_error.is_none(), "{}", result_text(&result));
    let text = result_text(&result);
    assert!(text.contains("Pass rate:"), "{text}");
    assert!(text.contains("#1 [PASS] MCP test experiment"), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_gameday_run_is_refused_while_an_enactment_is_in_flight() {
    let tmp = tempfile::tempdir().unwrap();
    crate::tools::test_support::write_valid_experiment(tmp.path());
    let handler = open_handler(tmp.path());
    create_runnable_gameday(&handler, "busy-gd").await;

    // Hold the server-wide enactment slot: the campaign must be refused
    // (tool-level error), never queued.
    let _guard = handler
        .enact_lock
        .try_acquire()
        .expect("the slot must be free before the call");
    let params = call_params(
        "tumult_gameday_run",
        serde_json::json!({ "gameday_path": "busy-gd.gameday.toon" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("a busy enactment is a tool-level error, not a protocol error");
    assert_eq!(result.is_error, Some(true));
    let text = result_text(&result);
    assert!(
        text.contains("another fault-injection enactment is already running"),
        "{text}"
    );
    assert!(
        !tmp.path().join("busy-gd.gameday.journal.toon").exists(),
        "a refused campaign must not run"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_gameday_list_searches_a_validated_subdirectory() {
    let tmp = tempfile::tempdir().unwrap();
    let nested = tmp.path().join("campaigns");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("x.gameday.toon"), "title: nested campaign\n").unwrap();
    let handler = open_handler(tmp.path());

    // An explicit `path` argument goes through resolve_path containment.
    let params = call_params(
        "tumult_gameday_list",
        serde_json::json!({ "path": "campaigns" }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("list must produce a result");
    assert!(result.is_error.is_none(), "{}", result_text(&result));
    let structured = result.structured_content.as_ref().unwrap();
    assert_eq!(structured["total"], 1);

    // A path escaping the workspace is a protocol-level argument error.
    let params = call_params(
        "tumult_gameday_list",
        serde_json::json!({ "path": "../escape" }),
        None,
    );
    let err = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect_err("an escaping path must be rejected before dispatch");
    assert!(err.to_string().contains("path"), "got: {err}");
}

/// A store and an enabled (playbook-less) policy in `tmp`.
fn store_and_policy(tmp: &tempfile::TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let store = tmp.path().join("analytics.duckdb");
    drop(tumult_lake::AnalyticsStore::open(&store).unwrap());
    let policy = tmp.path().join("autopilot.toml");
    std::fs::write(&policy, "[autopilot]\nenabled = true\n").unwrap();
    (store, policy)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_autopilot_run_execute_holds_the_enactment_slot() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, policy) = store_and_policy(&tmp);
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_autopilot_run",
        serde_json::json!({
            "policy_path": policy.to_str().unwrap(),
            "store_path": store.to_str().unwrap(),
            "execute": true,
        }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("run must produce a result");
    assert!(result.is_error.is_none(), "{}", result_text(&result));
    let structured = result.structured_content.as_ref().unwrap();
    assert_eq!(structured["executed"], true);
    // The pass held the slot itself and released it on completion.
    assert_eq!(handler.enact_lock.in_flight(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_autopilot_run_without_the_slot_gates_against_in_flight_count() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, policy) = store_and_policy(&tmp);
    let handler = open_handler(tmp.path());

    // Another enactment in flight: this pass cannot take the slot, so its
    // gate evaluation observes the in-flight count instead of 0.
    let _guard = handler.enact_lock.try_acquire().unwrap();
    let params = call_params(
        "tumult_autopilot_run",
        serde_json::json!({
            "policy_path": policy.to_str().unwrap(),
            "store_path": store.to_str().unwrap(),
            "execute": true,
        }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("the pass must still run (decide-and-record) without the slot");
    assert!(result.is_error.is_none(), "{}", result_text(&result));
    assert_eq!(
        handler.enact_lock.in_flight(),
        1,
        "the pass must not touch the other enactment's slot"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_autopilot_respond_approve_on_unknown_decision_is_a_tool_error() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, policy) = store_and_policy(&tmp);
    let handler = open_handler(tmp.path());

    // approve=true takes the enactment slot for the re-gate; the unknown
    // decision must surface as a tool-level NotFound, and the slot release.
    let params = call_params(
        "tumult_autopilot_respond",
        serde_json::json!({
            "decision_id": "no-such-decision",
            "approve": true,
            "policy_path": policy.to_str().unwrap(),
            "store_path": store.to_str().unwrap(),
        }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("an unknown decision is a tool-level error, not a protocol error");
    assert_eq!(result.is_error, Some(true));
    let text = result_text(&result);
    assert!(text.contains("no-such-decision"), "{text}");
    assert_eq!(handler.enact_lock.in_flight(), 0, "the slot must release");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tool_autopilot_notify_records_the_change_event() {
    let tmp = tempfile::tempdir().unwrap();
    let (store, _policy) = store_and_policy(&tmp);
    let handler = open_handler(tmp.path());

    let params = call_params(
        "tumult_autopilot_notify",
        serde_json::json!({
            "service": "db",
            "source": "deploy-webhook",
            "detail": "v2 rollout",
            "store_path": store.to_str().unwrap(),
        }),
        None,
    );
    let result = handler
        .handle_call_tool_request(params, stub_runtime())
        .await
        .expect("notify must produce a result");
    assert!(result.is_error.is_none(), "{}", result_text(&result));
    let structured = result.structured_content.as_ref().unwrap();
    assert_eq!(structured["service"], "db");
    assert_eq!(structured["source"], "deploy-webhook");
    assert!(result_text(&result).contains("change event recorded"));
}
