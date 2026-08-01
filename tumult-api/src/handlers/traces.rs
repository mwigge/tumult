//! `GET /api/traces` (+ `/durations`, `/{id}`).

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::sql_util::{and_pred, attr_kv, env_trace_exists, sql_string, w, windows, with_reader};
use crate::ApiState;

// ---------------------------------------------------------------------------
// GET /api/traces + /api/traces/durations + /api/traces/{id}

#[derive(Deserialize)]
pub(crate) struct TracesParams {
    range: Option<String>,
    service: Option<String>,
    min_duration_ms: Option<f64>,
    outcome: Option<String>,
    attr: Option<String>,
    attr_not: Option<String>,
}

/// Reusable outcome predicate on the joined `experiment.completed` status
/// (`l` must be the logs alias of that join); `None` passes everything.
fn outcome_where(outcome: Option<&str>) -> Result<Option<String>, String> {
    let Some(outcome) = outcome.filter(|o| !o.is_empty()) else {
        return Ok(None);
    };
    let lower = outcome.to_ascii_lowercase();
    match lower.as_str() {
        // tumult stores the status capitalised in log attributes.
        "completed" | "deviated" | "failed" => {
            let mut chars = lower.chars();
            let cap = chars.next().map_or_else(String::new, |f| {
                f.to_uppercase().collect::<String>() + chars.as_str()
            });
            Ok(Some(format!(
                "l.log_attrs['status'] = {}",
                sql_string(&cap)
            )))
        }
        "incomplete" => Ok(Some("l.log_attrs['status'] IS NULL".into())),
        other => Err(format!(
            "invalid outcome {other:?}; expected completed|deviated|failed|incomplete"
        )),
    }
}

pub(crate) async fn traces(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<TracesParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();

    // Span-level window (traces are grouped only from spans inside it).
    let mut span_wheres = Vec::new();
    if let Some(range) = &params.range {
        let Some((cur, _)) = windows(range) else {
            return Err(bad(format!("invalid range {range:?}; expected 24h|7d|14d")));
        };
        span_wheres.push(w(cur.0, cur.1));
    }
    let span_where = if span_wheres.is_empty() {
        "TRUE".to_string()
    } else {
        span_wheres.join(" AND ")
    };

    // Trace-level filters (they must not shrink the span set being grouped).
    let mut trace_wheres = Vec::new();
    // Scoped principals see only traces carrying an in-scope environment.
    if let Some(exists) = env_trace_exists("t", &principal.env_scopes) {
        trace_wheres.push(exists);
    }
    if let Some(service) = params.service.as_deref().filter(|s| !s.is_empty()) {
        trace_wheres.push(format!(
            "EXISTS (SELECT 1 FROM spans sx WHERE sx.trace_id = t.trace_id \
             AND sx.service_name = {})",
            sql_string(service)
        ));
    }
    if let Some(min_ms) = params.min_duration_ms {
        if !(min_ms.is_finite() && min_ms >= 0.0) {
            return Err(bad("min_duration_ms must be a non-negative number".into()));
        }
        trace_wheres.push(format!("t.duration_ns >= {}", (min_ms * 1e6) as i64));
    }
    if let Some(ow) = outcome_where(params.outcome.as_deref()).map_err(bad)? {
        trace_wheres.push(ow);
    }
    // Click-to-filter on span attributes (any span of the trace).
    for (raw, negate) in [
        (params.attr.as_deref(), false),
        (params.attr_not.as_deref(), true),
    ] {
        let Some(raw) = raw.filter(|s| !s.is_empty()) else {
            continue;
        };
        let Some((k, v)) = attr_kv(raw) else {
            return Err(bad(format!("invalid attr filter {raw:?}; expected k=v")));
        };
        let not = if negate { "NOT " } else { "" };
        trace_wheres.push(format!(
            "{not}EXISTS (SELECT 1 FROM spans sx WHERE sx.trace_id = t.trace_id \
             AND (sx.span_attrs[{k}] = {v} OR sx.resource_attrs[{k}] = {v}))",
            k = sql_string(k),
            v = sql_string(v)
        ));
    }
    let trace_where = if trace_wheres.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", trace_wheres.join(" AND "))
    };

    // Group spans into traces. The display root is the (earliest) span with
    // no parent; traces without one fall back to the earliest span. Only
    // experiment runs have an outcome, joined through the completed log.
    let sql = format!(
        "SELECT t.trace_id, t.started_ns, t.duration_ns, t.span_count, t.error_count, \
         COALESCE(r.span_name, t.first_span) AS root_name, \
         COALESCE(r.service_name, t.first_service) AS service_name, \
         t.experiment_id, t.experiment_name, \
         l.log_attrs['status'] AS status \
         FROM ( \
           SELECT trace_id, MIN(ts_ns) AS started_ns, \
                  MAX(ts_ns + duration_ns) - MIN(ts_ns) AS duration_ns, \
                  COUNT(*) AS span_count, \
                  COUNT(*) FILTER (WHERE status_code = 'Error') AS error_count, \
                  MAX(experiment_id) AS experiment_id, \
                  MAX(experiment_name) AS experiment_name, \
                  arg_min(span_name, ts_ns) AS first_span, \
                  arg_min(service_name, ts_ns) AS first_service \
           FROM spans WHERE {span_where} GROUP BY trace_id \
         ) t \
         LEFT JOIN ( \
           SELECT trace_id, span_name, service_name, \
                  ROW_NUMBER() OVER (PARTITION BY trace_id ORDER BY ts_ns) AS rn \
           FROM spans WHERE parent_span_id IS NULL \
         ) r ON r.trace_id = t.trace_id AND r.rn = 1 \
         LEFT JOIN logs l ON l.log_attrs['experiment_id'] = t.experiment_id \
            AND l.body = 'experiment.completed' \
         {trace_where} ORDER BY t.started_ns DESC LIMIT 200"
    );
    let rows = with_reader(&state.db_path, move |reader| {
        reader.query_json_rows(&sql).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "traces": rows})))
}

#[derive(Deserialize)]
pub(crate) struct TraceDurationsParams {
    range: Option<String>,
}

pub(crate) async fn trace_durations(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<TraceDurationsParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let mut span_where = "parent_span_id IS NULL".to_string();
    if let Some(exists) = env_trace_exists("spans", &principal.env_scopes) {
        span_where.push_str(&format!(" AND {exists}"));
    }
    if let Some(range) = &params.range {
        let Some((cur, _)) = windows(range) else {
            return Err(bad(format!("invalid range {range:?}; expected 24h|7d|14d")));
        };
        span_where.push_str(&format!(" AND {}", w(cur.0, cur.1)));
    }
    let body = with_reader(&state.db_path, move |reader| {
        let points = reader
            .query_json_rows(&format!(
                "SELECT trace_id, ts_ns, duration_ns / 1000000.0 AS duration_ms \
                 FROM spans WHERE {span_where} ORDER BY ts_ns LIMIT 1000"
            ))
            .map_err(|e| e.to_string())?;
        let pct = reader
            .query_json_rows(&format!(
                "SELECT quantile_cont(duration_ns, 0.5) / 1000000.0 AS p50, \
                 quantile_cont(duration_ns, 0.95) / 1000000.0 AS p95, \
                 quantile_cont(duration_ns, 0.99) / 1000000.0 AS p99 \
                 FROM spans WHERE {span_where}"
            ))
            .map_err(|e| e.to_string())?;
        let pct = pct.into_iter().next().unwrap_or_else(|| json!({}));
        Ok(json!({
            "points": points,
            "p50_ms": pct.get("p50").cloned().unwrap_or(Value::Null),
            "p95_ms": pct.get("p95").cloned().unwrap_or(Value::Null),
            "p99_ms": pct.get("p99").cloned().unwrap_or(Value::Null),
        }))
    })
    .await?;
    Ok(Json(body))
}

pub(crate) async fn trace_detail(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    if id.chars().count() > 200 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "trace id too long"})),
        )
            .into_response());
    }
    // Traces outside the principal's scopes look exactly like a missing
    // trace (404 — no existence leak across scopes).
    let env = and_pred(env_trace_exists("spans", &principal.env_scopes));
    let body = with_reader(&state.db_path, move |reader| {
        let id_sql = sql_string(&id);
        let spans = reader
            .query_json_rows(&format!(
                "SELECT ts_ns, trace_id, span_id, parent_span_id, span_name, span_kind, \
                 duration_ns, status_code, status_message, service_name, \
                 experiment_id, experiment_name, fault_type, fault_subtype, \
                 span_attrs, events \
                 FROM spans WHERE trace_id = {id_sql}{env} ORDER BY ts_ns LIMIT 2000"
            ))
            .map_err(|e| e.to_string())?;
        if spans.is_empty() {
            return Ok(None);
        }
        let logs = reader
            .query_json_rows(&format!(
                "SELECT ts_ns, severity_text, body, trace_id, span_id, log_attrs \
                 FROM logs WHERE trace_id = {id_sql} ORDER BY ts_ns LIMIT 1000"
            ))
            .map_err(|e| e.to_string())?;
        Ok(Some(json!({
            "trace_id": id,
            "spans": spans,
            "logs": logs,
        })))
    })
    .await?;
    match body {
        Some(body) => Ok(Json(body)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "trace not found"})),
        )
            .into_response()),
    }
}
