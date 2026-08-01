//! `GET /api/topology` — service/target call graph.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::sql_util::{and_pred, env_trace_exists, w, windows, with_reader};
use crate::ApiState;

// ---------------------------------------------------------------------------
// GET /api/topology

#[derive(Deserialize)]
pub(crate) struct TopologyParams {
    range: Option<String>,
}

/// tumult tags spans with the system under test under this attribute.
const TARGET_ATTR: &str = "span_attrs['resilience.target.name']";

pub(crate) async fn topology(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<TopologyParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let mut window = String::new();
    let mut cwindow = String::new();
    if let Some(range) = &params.range {
        let Some((cur, _)) = windows(range) else {
            return Err(bad(format!("invalid range {range:?}; expected 24h|7d|14d")));
        };
        window = format!(" AND {}", w(cur.0, cur.1));
        // Edge queries join spans to itself — qualify the window there.
        cwindow = format!(" AND c.ts_ns >= {} AND c.ts_ns < {}", cur.0, cur.1);
    }
    // Scoped principals see only the graph of their own environments.
    window.push_str(&and_pred(env_trace_exists("spans", &principal.env_scopes)));
    cwindow.push_str(&and_pred(env_trace_exists("c", &principal.env_scopes)));
    let body = with_reader(&state.db_path, move |reader| {
        let query = |sql: &str| reader.query_json_rows(sql).map_err(|e| e.to_string());
        // Nodes: one per service and one per tumult target, with health
        // aggregates over their spans.
        let mut nodes = query(&format!(
            "SELECT service_name AS name, 'service' AS type, COUNT(*) AS runs, \
             COUNT(*) FILTER (WHERE status_code = 'Error') AS errors, \
             AVG(duration_ns) AS avg_duration_ns \
             FROM spans WHERE service_name <> ''{window} GROUP BY 1"
        ))?;
        nodes.extend(query(&format!(
            "SELECT {TARGET_ATTR} AS name, 'target' AS type, COUNT(*) AS runs, \
             COUNT(*) FILTER (WHERE status_code = 'Error') AS errors, \
             AVG(duration_ns) AS avg_duration_ns \
             FROM spans WHERE {TARGET_ATTR} IS NOT NULL{window} GROUP BY 1"
        ))?);

        // Edges: parent→child service hops (a differing span name keeps
        // intra-service calls visible as self-loops), plus service→target
        // calls from the target attribute.
        let mut edges = query(&format!(
            "SELECT 'svc:' || p.service_name AS from_id, 'svc:' || c.service_name AS to_id, \
             COUNT(*) AS weight \
             FROM spans c JOIN spans p ON c.parent_span_id = p.span_id \
             WHERE (p.service_name <> c.service_name OR p.span_name <> c.span_name){cwindow} \
             GROUP BY 1, 2"
        ))?;
        edges.extend(query(&format!(
            "SELECT 'svc:' || service_name AS from_id, \
             'tgt:' || {TARGET_ATTR} AS to_id, COUNT(*) AS weight \
             FROM spans WHERE {TARGET_ATTR} IS NOT NULL{window} GROUP BY 1, 2"
        ))?);

        // Busiest first, capped at 100 nodes; edges to trimmed nodes drop.
        nodes
            .sort_by_key(|n| std::cmp::Reverse(n.get("runs").and_then(Value::as_u64).unwrap_or(0)));
        nodes.truncate(100);
        let nodes: Vec<Value> = nodes
            .into_iter()
            .filter_map(|n| {
                let name = n.get("name").and_then(Value::as_str)?;
                let kind = n.get("type").and_then(Value::as_str)?;
                let prefix = if kind == "service" { "svc" } else { "tgt" };
                Some(json!({
                    "id": format!("{prefix}:{name}"),
                    "name": name,
                    "type": kind,
                    "runs": n.get("runs").cloned().unwrap_or(Value::Null),
                    "errors": n.get("errors").cloned().unwrap_or(Value::Null),
                    "avg_duration_ns": n.get("avg_duration_ns").cloned().unwrap_or(Value::Null),
                }))
            })
            .collect();
        let ids: std::collections::HashSet<&str> = nodes
            .iter()
            .filter_map(|n| n.get("id").and_then(Value::as_str))
            .collect();
        let edges: Vec<Value> = edges
            .into_iter()
            .filter(|e| {
                let from = e.get("from_id").and_then(Value::as_str).unwrap_or("");
                let to = e.get("to_id").and_then(Value::as_str).unwrap_or("");
                ids.contains(from) && ids.contains(to)
            })
            .collect();
        Ok(json!({"nodes": nodes, "edges": edges}))
    })
    .await?;
    Ok(Json(body))
}
