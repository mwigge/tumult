// Imported from kronika. Pedantic lints are scoped to tumult-native
// crates; this file predates the pedantic gate (see crate lib.rs).
#![allow(clippy::pedantic)]

//! Daemon-side configuration: `ApiState::from_env_parts` loads the org tree
//! and the autopilot policy from `KRONIKA_*` env vars (falling back
//! closed/safe on bad input), and `POST /api/lake/export` enforces retention
//! only when it is configured *and* the ingest handle is wired.
//!
//! These tests mutate process env, so they live in their own integration
//! binary and serialise on `ENV_LOCK`.

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::StatusCode;
use tumult_api::ApiState;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn metrics_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../metrics")
        .canonicalize()
        .unwrap()
}

#[tokio::test]
async fn from_env_parts_loads_org_and_policy_falling_back_safely() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("k.duckdb");
    let _store = tumult_lake::Store::open(&db_path).unwrap();

    // No env: empty org tree, no policy, reports under the db dir.
    std::env::remove_var("KRONIKA_ORG_FILE");
    std::env::remove_var("KRONIKA_AUTOPILOT_POLICY");
    let state = ApiState::from_env_parts(db_path.clone(), metrics_dir(), None, None, true);
    assert_eq!(state.reports_dir(), &tmp.path().join("reports"));
    assert!(state.autopilot_policy().is_none());
    assert!(state.ingest_handle().is_none());
    assert!(state.runs_handle().is_none());
    assert!(state.secure_cookies());

    // A valid org file loads — its nodes resolve through the scores tree.
    let org_path = tmp.path().join("org.yaml");
    std::fs::write(&org_path, "nodes: [{name: platform, kind: domain}]").unwrap();
    std::env::set_var("KRONIKA_ORG_FILE", &org_path);
    let state = ApiState::from_env_parts(db_path.clone(), metrics_dir(), None, None, false);
    assert!(!state.secure_cookies());
    let base = serve(state).await;
    let resp = reqwest::get(format!("{base}/api/scores/tree?node=platform"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // An invalid org file falls back to the empty tree: only the implicit
    // root and (unassigned) resolve.
    std::fs::write(&org_path, "nodes: [{name: a}, {name: a}]").unwrap();
    let state = ApiState::from_env_parts(db_path.clone(), metrics_dir(), None, None, false);
    let base = serve(state).await;
    let resp = reqwest::get(format!("{base}/api/scores/tree?node=platform"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let resp = reqwest::get(format!("{base}/api/scores/tree"))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // A valid policy loads; an invalid one fails closed (None).
    let policy_path = tmp.path().join("policy.toml");
    std::fs::write(&policy_path, "[autopilot]\nenabled = true\n").unwrap();
    std::env::set_var("KRONIKA_AUTOPILOT_POLICY", &policy_path);
    let state = ApiState::from_env_parts(db_path.clone(), metrics_dir(), None, None, false);
    assert!(state.autopilot_policy().is_some());
    std::fs::write(&policy_path, "autopilot = [").unwrap();
    let state = ApiState::from_env_parts(db_path, metrics_dir(), None, None, false);
    assert!(state.autopilot_policy().is_none());

    std::env::remove_var("KRONIKA_ORG_FILE");
    std::env::remove_var("KRONIKA_AUTOPILOT_POLICY");
}

async fn serve(state: ApiState) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, tumult_api::router(state))
            .await
            .unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn retention_export_requires_the_ingest_handle() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("k.duckdb");
    let store = tumult_lake::Store::open(&db_path).unwrap();
    std::env::set_var("KRONIKA_LAKE_DIR", tmp.path().join("lake"));
    std::env::set_var("KRONIKA_RETENTION_DAYS", "1");

    // Without the daemon's ingest handle, a retention-configured export is
    // refused (503) rather than silently skipping deletion.
    let state = ApiState::new(
        db_path.clone(),
        metrics_dir(),
        tmp.path().join("reports"),
        Arc::new(tumult_intelligence::llm::OpenAiCompatClient::from_env()),
        tumult_compliance::OrgTree::empty(),
        None,
        None,
        None,
        false,
    );
    let err = tumult_api::lake::export_now(axum::extract::State(state))
        .await
        .unwrap_err();
    assert_eq!(err.status(), StatusCode::SERVICE_UNAVAILABLE);

    // With the handle wired, the export runs and reports what retention
    // deleted (nothing, on an empty store).
    let (ingest, _task) = tumult_ingest::IngestWriter::spawn(store.writer().unwrap(), 4);
    let state = ApiState::new(
        db_path,
        metrics_dir(),
        tmp.path().join("reports"),
        Arc::new(tumult_intelligence::llm::OpenAiCompatClient::from_env()),
        tumult_compliance::OrgTree::empty(),
        Some(ingest),
        None,
        None,
        false,
    );
    let axum::Json(body) = tumult_api::lake::export_now(axum::extract::State(state))
        .await
        .unwrap();
    assert!(body["deleted"].is_object(), "{body}");
    assert!(body["tables"].is_array(), "{body}");

    std::env::remove_var("KRONIKA_LAKE_DIR");
    std::env::remove_var("KRONIKA_RETENTION_DAYS");
}

#[tokio::test]
async fn lake_status_reports_watermarks_and_policy() {
    let _guard = ENV_LOCK.lock().await;
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("k.duckdb");
    let _store = tumult_lake::Store::open(&db_path).unwrap();
    std::env::set_var("KRONIKA_LAKE_DIR", tmp.path().join("lake"));
    std::env::remove_var("KRONIKA_RETENTION_DAYS");

    let state = ApiState::new(
        db_path,
        metrics_dir(),
        tmp.path().join("reports"),
        Arc::new(tumult_intelligence::llm::OpenAiCompatClient::from_env()),
        tumult_compliance::OrgTree::empty(),
        None,
        None,
        None,
        false,
    );
    let axum::Json(body) = tumult_api::lake::status(axum::extract::State(state))
        .await
        .unwrap();
    assert!(body["watermarks"].is_object(), "{body}");
    assert_eq!(body["retention_days"], serde_json::json!(0));

    std::env::remove_var("KRONIKA_LAKE_DIR");
}
