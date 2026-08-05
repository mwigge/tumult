//! Daemon self-observability endpoints: deep `GET /healthz` (liveness),
//! `GET /readyz` (readiness) and `GET /metrics` (Prometheus text).
//!
//! Mounted under the API's auth middleware in `serve.rs` — the same pattern
//! as `/report`, gated at Viewer by `tumult_api::auth::ROUTE_TABLE`. While
//! the store has no real users the middleware is open, so loopback probes
//! keep working on unprovisioned installs; k8s' probe contract (any 2xx/3xx
//! — and 401 — is "alive") still holds once auth is on.

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use tumult_ingest::{daemon_metrics, IngestWriter};
use tumult_lake::Store;

/// Bound on a single probe round-trip: a wedged writer task must fail the
/// probe fast, not hang the probe handler.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct OpsState {
    pub db_path: PathBuf,
    pub ingest: IngestWriter,
}

/// The ops router. Layer the API auth middleware on it at merge time.
pub fn router(state: OpsState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
        .with_state(state)
}

/// Store probe: open a fresh read-only connection (coexists with the
/// daemon's single writer) and round-trip a trivial query. Blocking — call
/// inside `spawn_blocking`.
fn probe_store(db_path: &Path) -> Result<(), String> {
    Store::at(db_path)
        .read_only()
        .map_err(|e| e.to_string())?
        .query_json_rows("SELECT 1 AS ok")
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// The store's schema version (readiness: migrations applied).
fn schema_version(db_path: &Path) -> Result<i64, String> {
    Store::at(db_path)
        .read_only()
        .map_err(|e| e.to_string())?
        .query_json_rows("SELECT value FROM schema_meta WHERE key = 'version'")
        .map_err(|e| e.to_string())?
        .first()
        .and_then(|r| r["value"].as_i64())
        .ok_or_else(|| "schema_meta has no version row".to_string())
}

/// Writer-channel liveness: a no-op round-trip proves the writer task is
/// alive and processing.
async fn probe_writer(ingest: &IngestWriter) -> Result<(), String> {
    match tokio::time::timeout(PROBE_TIMEOUT, ingest.ping()).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("writer channel: {e}")),
        Err(_) => Err("writer channel: probe timed out".into()),
    }
}

async fn probe_store_async(db_path: &Path) -> Result<(), String> {
    let db = db_path.to_path_buf();
    tokio::task::spawn_blocking(move || probe_store(&db))
        .await
        .map_err(|e| format!("store probe task: {e}"))?
}

fn unavailable(reason: &str) -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, reason.to_string()).into_response()
}

/// `GET /healthz` — liveness: the writer channel round-trips and the store
/// answers. 200 `ok` or 503 with the failing probe.
async fn healthz(State(state): State<OpsState>) -> Response {
    if let Err(e) = probe_writer(&state.ingest).await {
        return unavailable(&e);
    }
    if let Err(e) = probe_store_async(&state.db_path).await {
        return unavailable(&format!("store: {e}"));
    }
    (StatusCode::OK, "ok").into_response()
}

/// `GET /readyz` — readiness: liveness plus migrations applied and at least
/// one supervisor tick since boot (a dead supervisor task stops ticking).
async fn readyz(State(state): State<OpsState>) -> Response {
    if let Err(e) = probe_writer(&state.ingest).await {
        return unavailable(&e);
    }
    let db = state.db_path.clone();
    let version = tokio::task::spawn_blocking(move || schema_version(&db))
        .await
        .map_err(|e| format!("schema probe task: {e}"))
        .and_then(std::convert::identity);
    match version {
        Ok(v) if v == tumult_lake::CURRENT_SCHEMA_VERSION => {}
        Ok(v) => {
            return unavailable(&format!(
                "schema version {v} (expected {})",
                tumult_lake::CURRENT_SCHEMA_VERSION
            ));
        }
        Err(e) => return unavailable(&format!("schema: {e}")),
    }
    if daemon_metrics::supervisor_last_tick_ns() == 0 {
        return unavailable("supervisor has not ticked yet");
    }
    (StatusCode::OK, "ready").into_response()
}

/// `GET /metrics` — the daemon's own SLIs in Prometheus text format.
async fn metrics() -> Response {
    (
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        daemon_metrics::render_prometheus(),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn live_state() -> (tempfile::TempDir, OpsState, tokio::task::JoinHandle<()>) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("kronika.duckdb");
        let store = Store::open(&db_path).unwrap();
        let (ingest, task) = IngestWriter::spawn(store.writer().unwrap(), 8);
        let state = OpsState { db_path, ingest };
        (dir, state, task)
    }

    #[tokio::test]
    async fn healthz_is_ok_with_a_live_writer_and_store() {
        let (_dir, state, _task) = live_state().await;
        let resp = healthz(State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn healthz_fails_when_the_writer_task_is_dead() {
        let (_dir, state, task) = live_state().await;
        task.abort(); // dropping the receiver: every write now fails
                      // Give the abort a beat to take effect.
        tokio::task::yield_now().await;
        let resp = healthz(State(state)).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn healthz_fails_when_the_store_is_gone() {
        let (_dir, mut state, _task) = live_state().await;
        state.db_path = PathBuf::from("/nonexistent/store.duckdb");
        let resp = healthz(State(state)).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn readyz_is_ready_with_current_schema_and_a_supervisor_tick() {
        let (_dir, state, _task) = live_state().await;
        daemon_metrics::supervisor_tick();
        let resp = readyz(State(state)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn metrics_renders_prometheus_text() {
        daemon_metrics::run_started();
        let resp = metrics().await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("tumultd_runs_started_total 1"), "{text}");
        assert!(
            text.contains("# TYPE tumultd_active_campaigns gauge"),
            "{text}"
        );
    }
}
