//! `GET /api/lake/status` and `POST /api/lake/export` — the parquet lake's
//! observability and its manual trigger (the scheduled job in `kronikad`
//! runs the same `tumult_lake::lake::export` on an interval).

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

/// `POST /api/lake/export` — commit a portable snapshot. Automatic deletion
/// is unavailable until historical reports support archive queries.
pub async fn export_now(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let cfg = cfg(&state);
    if cfg.retention_days > 0 {
        return Err((StatusCode::SERVICE_UNAVAILABLE, Json(json!({
            "error": "KRONIKA_RETENTION_DAYS must be 0: historical queries do not yet read archived snapshots; no data was deleted"
        }))).into_response());
    }
    let report = with_reader(&state.db_path, move |reader| {
        lake::export(reader, &cfg).map_err(|e| e.to_string())
    })
    .await?;
    let mut body = serde_json::to_value(report).map_err(|e| internal(e.to_string()))?;
    body["deleted"] = json!({});
    Ok(Json(body))
}
