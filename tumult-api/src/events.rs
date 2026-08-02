//! Event feed endpoint (`GET /api/events`) — the cross-run audit feed over
//! the hash-chained `run_audit` table (schema v7), newest first, with the
//! definition name joined. The chain links (`prev_hash` / `new_hash`) ride
//! along so the UI can show the trail is tamper-evident; per-run chain
//! verification stays on `/api/runs/{id}/audit/verify`.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::sql_util::{sql_string, with_reader};
use crate::ApiState;

fn bad_request(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response()
}

/// Query params: `run_id` and `event` are exact-match filters, `limit`
/// (default 100, cap 200) bounds the page, `before` is the cursor (only
/// events older than this `at_ns`).
#[derive(Debug, Deserialize)]
pub struct ListParams {
    run_id: Option<String>,
    event: Option<String>,
    limit: Option<u32>,
    before: Option<i64>,
}

/// `GET /api/events` — every run's audit events, newest first. Scoped
/// principals see only runs in their environments (same rule as the run
/// list: runs without a linked experiment stay visible to everyone).
pub async fn list(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, Response> {
    if params
        .run_id
        .as_deref()
        .is_some_and(|id| id.chars().count() > 100)
    {
        return Err(bad_request("run id too long".into()));
    }
    if params
        .event
        .as_deref()
        .is_some_and(|e| e.chars().count() > 100)
    {
        return Err(bad_request("event name too long".into()));
    }
    let limit = params.limit.unwrap_or(100).clamp(1, 200);
    let scopes = principal.env_scopes.clone();
    let rows = with_reader(&state.db_path, move |reader| {
        let mut clauses = Vec::new();
        if let Some(run_id) = params.run_id.as_deref().filter(|s| !s.is_empty()) {
            clauses.push(format!("a.run_id = {}", sql_string(run_id)));
        }
        if let Some(event) = params.event.as_deref().filter(|s| !s.is_empty()) {
            clauses.push(format!("a.event = {}", sql_string(event)));
        }
        if let Some(before) = params.before {
            clauses.push(format!("a.at_ns < {before}"));
        }
        if !scopes.is_empty() {
            let env_list = scopes
                .iter()
                .map(|s| sql_string(s))
                .collect::<Vec<_>>()
                .join(", ");
            clauses.push(format!(
                "(e.env IN ({env_list}) OR r.experiment_id IS NULL)"
            ));
        }
        let where_clause = if clauses.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", clauses.join(" AND "))
        };
        reader
            .query_json_rows(&format!(
                "SELECT a.*, g.name AS definition_name FROM run_audit a \
                 LEFT JOIN runs r ON r.id = a.run_id \
                 LEFT JOIN run_registry g ON g.id = r.registry_id \
                 LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env \
                            FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id \
                 {where_clause} ORDER BY a.at_ns DESC LIMIT {limit}"
            ))
            .map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "events": rows})))
}
