//! `GET /api/lake/status` and `POST /api/lake/export` — the parquet lake's
//! observability and its manual trigger (the scheduled job in `kronikad`
//! runs the same `tumult_lake::lake::export` on an interval).

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};
use tumult_lake::lake::{self, LakeConfig};

use crate::sql_util::{internal, with_reader};
use crate::ApiState;

fn cfg(state: &ApiState) -> LakeConfig {
    LakeConfig::from_env(&state.db_path)
}

/// `GET /api/lake/status` — watermarks per table, file/byte totals, policy.
pub async fn status(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let cfg = cfg(&state);
    let status = tokio::task::spawn_blocking(move || lake::status(&cfg).map_err(|e| e.to_string()))
        .await
        .map_err(|e| internal(format!("lake status task failed: {e}")))?
        .map_err(internal)?;
    Ok(Json(
        serde_json::to_value(status).map_err(|e| internal(e.to_string()))?,
    ))
}

/// `POST /api/lake/export` — run one export pass now (same code path as the
/// scheduled job). Retention follows only when `KRONIKA_RETENTION_DAYS > 0`
/// and the ingest handle is wired (daemon); the audit table is never
/// deleted either way.
pub async fn export_now(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let cfg = cfg(&state);
    let report = with_reader(&state.db_path, {
        let cfg = cfg.clone();
        move |reader| lake::export(reader, &cfg).map_err(|e| e.to_string())
    })
    .await?;

    let mut deleted = json!({});
    if cfg.retention_days > 0 {
        let Some(ingest) = state.ingest_handle() else {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({"error": "retention needs the daemon's ingest handle (not wired)"})),
            )
                .into_response());
        };
        let slot = Arc::new(Mutex::new(None));
        let slot2 = Arc::clone(&slot);
        ingest
            .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
                *slot2.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(lake::enforce_retention(writer, &cfg).map_err(|e| e.to_string()));
                Ok(())
            })))
            .await
            .map_err(|e| internal(e.to_string()))?;
        match slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(Ok(d)) => {
                deleted = serde_json::to_value(d).map_err(|e| internal(e.to_string()))?;
            }
            Some(Err(e)) => return Err(internal(format!("retention failed: {e}"))),
            None => return Err(internal("retention did not run".into())),
        };
    }

    let mut body = serde_json::to_value(report).map_err(|e| internal(e.to_string()))?;
    body["deleted"] = deleted;
    Ok(Json(body))
}
