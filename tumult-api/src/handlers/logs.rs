//! `GET /api/logs` and `GET /api/logs/volume`.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::sql_util::{
    attr_wheres, bucket_expr, env_scope_where, parse_interval, sql_contains, sql_ieq, sql_string,
    w, windows, with_reader,
};
use crate::ApiState;

// ---------------------------------------------------------------------------
// GET /api/logs + /api/logs/volume

#[derive(Deserialize)]
pub(crate) struct LogsParams {
    range: Option<String>,
    severity: Option<String>,
    service: Option<String>,
    q: Option<String>,
    attr: Option<String>,
    attr_not: Option<String>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
pub(crate) struct LogsVolumeParams {
    range: Option<String>,
    interval: Option<String>,
    severity: Option<String>,
    service: Option<String>,
    q: Option<String>,
    attr: Option<String>,
    attr_not: Option<String>,
}

/// WHERE clauses shared by the logs list and volume endpoints (range always
/// applies, so the list is never empty). Scoped principals see only logs
/// correlated with an in-scope experiment (via trace or experiment_id).
#[allow(clippy::too_many_arguments)]
fn log_wheres(
    range: Option<&str>,
    severity: Option<&str>,
    service: Option<&str>,
    q: Option<&str>,
    attr: Option<&str>,
    attr_not: Option<&str>,
    scopes: &[String],
) -> Result<Vec<String>, String> {
    let range = range.unwrap_or("24h");
    let Some((cur, _)) = windows(range) else {
        return Err(format!("invalid range {range:?}; expected 24h|7d|14d"));
    };
    let mut wheres = vec![w(cur.0, cur.1)];
    if let Some(env) = env_scope_where("se.target_environment", scopes) {
        wheres.push(format!(
            "EXISTS (SELECT 1 FROM spans se WHERE {env} AND (\
             se.trace_id = logs.trace_id \
             OR se.experiment_id = logs.log_attrs['experiment_id']))"
        ));
    }
    if let Some(sev) = severity.filter(|s| !s.is_empty()) {
        // Severity texts are short tokens (INFO/WARN/ERROR…): case-insensitive
        // exact match, not a contains search.
        wheres.push(format!("severity_text ILIKE {} ESCAPE '\\'", sql_ieq(sev)));
    }
    if let Some(svc) = service.filter(|s| !s.is_empty()) {
        wheres.push(format!("service_name = {}", sql_string(svc)));
    }
    if let Some(q) = q.filter(|s| !s.is_empty()) {
        if q.chars().count() > 200 {
            return Err("q too long (max 200 chars)".into());
        }
        wheres.push(format!("body ILIKE {} ESCAPE '\\'", sql_contains(q)));
    }
    wheres.extend(attr_wheres(
        attr,
        attr_not,
        ("log_attrs", "resource_attrs"),
    )?);
    Ok(wheres)
}

pub(crate) async fn logs(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<LogsParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let wheres = log_wheres(
        params.range.as_deref(),
        params.severity.as_deref(),
        params.service.as_deref(),
        params.q.as_deref(),
        params.attr.as_deref(),
        params.attr_not.as_deref(),
        &principal.env_scopes,
    )
    .map_err(bad)?;
    let limit = params.limit.unwrap_or(200).clamp(1, 1000);
    let sql = format!(
        "SELECT ts_ns, severity_text, body, trace_id, span_id, service_name, \
         log_attrs['experiment_id'] AS experiment_id, log_attrs, resource_attrs \
         FROM logs WHERE {} ORDER BY ts_ns DESC LIMIT {limit}",
        wheres.join(" AND ")
    );
    let rows = with_reader(&state.db_path, move |reader| {
        reader.query_json_rows(&sql).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "logs": rows})))
}

pub(crate) async fn logs_volume(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<LogsVolumeParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let interval = params.interval.unwrap_or_else(|| "1h".into());
    let Some(bucket_s) = parse_interval(&interval) else {
        return Err(bad(format!(
            "invalid interval {interval:?}; expected 5m|1h|1d"
        )));
    };
    let wheres = log_wheres(
        params.range.as_deref(),
        params.severity.as_deref(),
        params.service.as_deref(),
        params.q.as_deref(),
        params.attr.as_deref(),
        params.attr_not.as_deref(),
        &principal.env_scopes,
    )
    .map_err(bad)?;
    // One row per (bucket, severity); the UI pivots into stacked series.
    let sql = format!(
        "SELECT {} AS ts, COALESCE(severity_text, 'UNKNOWN') AS severity, COUNT(*) AS count \
         FROM logs WHERE {} GROUP BY 1, 2 ORDER BY 1",
        bucket_expr(bucket_s),
        wheres.join(" AND ")
    );
    let rows = with_reader(&state.db_path, move |reader| {
        reader.query_json_rows(&sql).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({
        "interval": interval,
        "bucket_s": bucket_s,
        "rows": rows,
    })))
}
