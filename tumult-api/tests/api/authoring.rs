use crate::common::*;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// /api/authoring* — catalog + scaffold over HTTP. The handlers discover
// plugins through the standard search paths, so tests point
// TUMULT_PLUGIN_PATH at the workspace's real `plugins/` directory (every
// test sets the same value, so parallel test threads cannot race on it).

fn point_discovery_at_workspace_plugins() {
    let plugins = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../plugins")
        .canonicalize()
        .unwrap();
    std::env::set_var("TUMULT_PLUGIN_PATH", plugins);
}

async fn post(base: &str, path: &str, body: Value) -> (u16, Value) {
    let resp = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap();
    (status, body)
}

#[tokio::test]
async fn catalog_serves_the_live_plugin_catalog() {
    point_discovery_at_workspace_plugins();
    let srv = spawn_server().await;
    let (status, body) = get(&srv.base, "/api/authoring/catalog").await;
    assert_eq!(status, 200);
    assert!(body["action_count"].as_u64().unwrap() > 0, "{body}");
    let pause = body["domains"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|d| d["actions"].as_array().unwrap())
        .find(|a| a["plugin"] == "tumult-containers" && a["name"] == "pause-container");
    let pause = pause.unwrap_or_else(|| panic!("pause-container missing from {body}"));
    assert_eq!(pause["kind"], "action");
    assert!(pause["args"]
        .as_array()
        .unwrap()
        .iter()
        .any(|arg| arg["name"] == "container_id" && arg["required"] == true));
}

#[tokio::test]
async fn scaffold_returns_validated_toon() {
    point_discovery_at_workspace_plugins();
    let srv = spawn_server().await;
    let (status, body) = post(
        &srv.base,
        "/api/authoring/scaffold",
        json!({
            "plugin": "tumult-containers",
            "action": "pause-container",
            "args": {"container_id": "demo-postgres"},
            "target": "demo-postgres",
            "probe_command": "pg_isready -h demo-postgres",
            "probe_expect": "accepting connections",
        }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["action"], "tumult-containers::pause-container");
    assert_eq!(body["valid"], true, "{body}");
    let toon = body["toon"].as_str().unwrap();
    assert!(toon.contains("pause-container"), "{toon}");
    assert!(toon.contains("demo-postgres"), "{toon}");

    // The scaffolded TOON must pass the platform's own validate pipeline —
    // this is the bridge into registration the web UI relies on.
    let (status, body) = post(&srv.base, "/api/runs/validate", json!({"toon": toon})).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["valid"], true, "{body}");
    assert!(body["registry_id"].as_str().unwrap().starts_with("reg-"));
}

#[tokio::test]
async fn scaffold_reports_invalid_experiments_without_registering() {
    point_discovery_at_workspace_plugins();
    let srv = spawn_server().await;
    // An unclosed probe regex fails engine validation: the endpoint still
    // answers 200 with valid:false, and nothing reaches the registry.
    let (status, body) = post(
        &srv.base,
        "/api/authoring/scaffold",
        json!({
            "plugin": "tumult-containers",
            "action": "pause-container",
            "args": {"container_id": "demo-postgres"},
            "target": "demo-postgres",
            "probe_command": "echo hi",
            "probe_expect": "(unclosed",
        }),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["valid"], false, "{body}");
    assert!(body["validation_error"].as_str().unwrap().len() > 1);

    let (status, body) = get(&srv.base, "/api/registry").await;
    assert_eq!(status, 200);
    assert_eq!(body["count"], 0, "scaffold must persist nothing: {body}");
}

#[tokio::test]
async fn scaffold_rejects_unknown_actions_and_missing_plugin() {
    point_discovery_at_workspace_plugins();
    let srv = spawn_server().await;
    let (status, body) = post(
        &srv.base,
        "/api/authoring/scaffold",
        json!({
            "plugin": "tumult-containers",
            "action": "nuke-everything",
            "target": "x",
        }),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(body["error"].as_str().unwrap().contains("unknown action"));

    let (status, body) = post(
        &srv.base,
        "/api/authoring/scaffold",
        json!({"action": "pause-container", "target": "x"}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert!(body["error"].as_str().unwrap().contains("plugin::action"));
}
