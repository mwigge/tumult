// Imported from kronika. Pedantic lints are scoped to tumult-native
// crates; this file predates the pedantic gate (see crate lib.rs).
#![allow(clippy::pedantic)]

//! Unit tests for the small pure helpers of the query API (the former
//! inline `mod tests` of `lib.rs`, moved with the handlers they cover).

use axum::http::StatusCode;
use serde_json::{json, Value};
use tumult_api::handlers::metrics::{hist_quantile, valid_attr_key};
use tumult_api::sql_util::internal;

#[test]
fn hist_quantile_interpolates_within_bucket() {
    // 4 observations: 1 below 100, 2 in [100,200), 1 at/above 200.
    let counts = [1.0, 2.0, 1.0];
    let bounds = [100.0, 200.0];
    // Median falls halfway through the middle bucket.
    assert_eq!(hist_quantile(&counts, &bounds, 0.5), Some(150.0));
    // p30 lands just inside the middle bucket.
    assert_eq!(hist_quantile(&counts, &bounds, 0.3), Some(110.0));
    // p95 lands in the overflow bucket → clamps to the last bound.
    assert_eq!(hist_quantile(&counts, &bounds, 0.95), Some(200.0));
}

#[test]
fn hist_quantile_handles_empty_and_zero_buckets() {
    assert_eq!(hist_quantile(&[], &[], 0.5), None);
    assert_eq!(hist_quantile(&[0.0, 0.0], &[100.0], 0.5), None);
    // Zero-count leading buckets are skipped; the target then lands in
    // the overflow bucket and clamps to the last bound.
    assert_eq!(hist_quantile(&[0.0, 4.0], &[100.0], 0.5), Some(100.0));
}

#[test]
fn attr_key_charset_is_strict() {
    assert!(valid_attr_key("route"));
    assert!(valid_attr_key("http.route_v2"));
    assert!(!valid_attr_key(""));
    assert!(!valid_attr_key("Route"));
    assert!(!valid_attr_key("x';DROP"));
    assert!(!valid_attr_key("a b"));
}

/// 500 bodies are generic: store internals (schema, paths, DuckDB error
/// text) are logged server-side, never returned to the client.
#[tokio::test]
async fn internal_error_hides_store_details() {
    let resp = internal("duckdb: IO Error: cannot open /var/lib/tumult/k.duckdb".into());
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body, json!({"error": "internal error"}));
}

// ---------------------------------------------------------------------------
// /api/authoring* — catalog and scaffold helpers (pure functions over an
// in-memory FaultCatalog; the handlers only add plugin discovery on top).

use axum::Json;
use tumult_api::authoring::{catalog_json, scaffold_json, ScaffoldBody};
use tumult_authoring::{
    ActionKind, CatalogAction, CatalogArg, CatalogDomain, Domain, FaultCatalog,
};

/// One container domain with the pause action and a probe, mirroring the
/// curated `tumult-containers` entry of the live catalog (actions and
/// probes share a namespace there, so the fixture must too).
fn fixture_catalog() -> FaultCatalog {
    FaultCatalog {
        domains: vec![CatalogDomain {
            domain: Domain::Container,
            label: "Containers".into(),
            actions: vec![
                CatalogAction {
                    plugin: "tumult-containers".into(),
                    name: "pause-container".into(),
                    description: "Pause a running container (freeze all processes)".into(),
                    kind: ActionKind::Action,
                    args: vec![
                        CatalogArg {
                            name: "container_id".into(),
                            required: true,
                            description: "Target container id or name".into(),
                        },
                        CatalogArg {
                            name: "runtime".into(),
                            required: false,
                            description: "Container runtime (docker/podman)".into(),
                        },
                    ],
                },
                CatalogAction {
                    plugin: "tumult-containers".into(),
                    name: "container-status".into(),
                    description: "Check if a container is running".into(),
                    kind: ActionKind::Probe,
                    args: vec![CatalogArg {
                        name: "container_id".into(),
                        required: true,
                        description: "Target container id or name".into(),
                    }],
                },
            ],
        }],
    }
}

fn scaffold_body(req: Value) -> ScaffoldBody {
    serde_json::from_value(req).unwrap()
}

#[test]
fn catalog_json_mirrors_the_mcp_tool_shape() {
    let body = catalog_json(&fixture_catalog());
    assert_eq!(body["action_count"], json!(2));
    let action = &body["domains"][0]["actions"][0];
    assert_eq!(action["plugin"], json!("tumult-containers"));
    assert_eq!(action["name"], json!("pause-container"));
    assert_eq!(action["kind"], json!("action"));
    assert_eq!(action["args"][0]["name"], json!("container_id"));
    assert_eq!(action["args"][0]["required"], json!(true));
    assert_eq!(body["domains"][0]["actions"][1]["kind"], json!("probe"));
}

#[test]
fn catalog_json_empty_catalog_is_a_valid_response() {
    let body = catalog_json(&FaultCatalog { domains: vec![] });
    assert_eq!(body, json!({"action_count": 0, "domains": []}));
}

#[test]
fn scaffold_json_builds_a_valid_experiment() {
    let body = scaffold_json(
        &fixture_catalog(),
        &scaffold_body(json!({
            "plugin": "tumult-containers",
            "action": "pause-container",
            "args": {"container_id": "demo-postgres", "runtime": "docker"},
            "target": "demo-postgres",
            "probe_command": "pg_isready -h demo-postgres",
            "probe_expect": "accepting connections",
            "title": "Pause postgres",
        })),
    )
    .unwrap();
    assert_eq!(body["action"], json!("tumult-containers::pause-container"));
    assert_eq!(body["valid"], json!(true), "{body}");
    assert!(body.get("validation_error").is_none());
    let toon = body["toon"].as_str().unwrap();
    assert!(toon.contains("Pause postgres"), "{toon}");
    assert!(toon.contains("demo-postgres"), "{toon}");
}

#[test]
fn scaffold_json_stringifies_non_string_args() {
    let body = scaffold_json(
        &fixture_catalog(),
        &scaffold_body(json!({
            "action": "tumult-containers::pause-container",
            "args": {"container_id": 42},
            "target": "demo-postgres",
        })),
    )
    .unwrap();
    assert_eq!(body["valid"], json!(true), "{body}");
}

#[test]
fn scaffold_json_defaults_title_and_probe() {
    let body = scaffold_json(
        &fixture_catalog(),
        &scaffold_body(json!({
            "plugin": "tumult-containers",
            "action": "pause-container",
            "target": "demo-postgres",
        })),
    )
    .unwrap();
    assert_eq!(body["valid"], json!(true), "{body}");
    let toon = body["toon"].as_str().unwrap();
    assert!(toon.contains("pause-container — demo-postgres"), "{toon}");
}

#[test]
fn scaffold_json_supports_http_probes() {
    let body = scaffold_json(
        &fixture_catalog(),
        &scaffold_body(json!({
            "plugin": "tumult-containers",
            "action": "pause-container",
            "target": "demo-app",
            "probe_url": "http://demo-app:8080/health",
            "probe_expect": "ok",
        })),
    )
    .unwrap();
    assert_eq!(body["valid"], json!(true), "{body}");
    assert!(body["toon"].as_str().unwrap().contains("demo-app:8080"));
}

#[test]
fn scaffold_json_reports_an_invalid_probe_as_invalid() {
    let err = scaffold_json(
        &fixture_catalog(),
        &scaffold_body(json!({
            "plugin": "tumult-containers",
            "action": "pause-container",
            "target": "x",
            "probe_command": "echo hi",
            "probe_expect": "(unclosed",
        })),
    )
    .unwrap();
    assert_eq!(err["valid"], json!(false));
    assert!(err["validation_error"].as_str().unwrap().len() > 1);
}

#[test]
fn scaffold_json_requires_plugin_or_qualified_action() {
    let (status, Json(body)) = scaffold_json(
        &fixture_catalog(),
        &scaffold_body(json!({"action": "pause-container", "target": "x"})),
    )
    .unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("plugin::action"));
}

#[test]
fn scaffold_json_rejects_actions_outside_the_catalog() {
    let (status, Json(body)) = scaffold_json(
        &fixture_catalog(),
        &scaffold_body(json!({
            "plugin": "tumult-containers",
            "action": "nuke-everything",
            "target": "x",
        })),
    )
    .unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("unknown action"));
}

/// Catalog probes are steady-state checks, not faults: scaffolding one as
/// the experiment's action would validate and register a semantically
/// wrong experiment, so it's a 400 just like an unknown name.
#[test]
fn scaffold_json_rejects_probes_as_actions() {
    let (status, Json(body)) = scaffold_json(
        &fixture_catalog(),
        &scaffold_body(json!({
            "plugin": "tumult-containers",
            "action": "container-status",
            "args": {"container_id": "demo-postgres"},
            "target": "demo-postgres",
        })),
    )
    .unwrap_err();
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body["error"].as_str().unwrap().contains("unknown action"));
}

#[test]
fn scaffold_json_prefers_probe_url_over_probe_command() {
    let body = scaffold_json(
        &fixture_catalog(),
        &scaffold_body(json!({
            "plugin": "tumult-containers",
            "action": "pause-container",
            "target": "demo-app",
            "probe_url": "http://demo-app:8080/health",
            "probe_command": "echo should-not-appear",
        })),
    )
    .unwrap();
    assert_eq!(body["valid"], json!(true), "{body}");
    let toon = body["toon"].as_str().unwrap();
    assert!(
        toon.contains("curl -fsS 'http://demo-app:8080/health'"),
        "{toon}"
    );
    assert!(!toon.contains("should-not-appear"), "{toon}");
}
