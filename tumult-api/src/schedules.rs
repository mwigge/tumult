//! Schedule CRUD endpoints (`/api/schedules*`) — interval-based recurring
//! runs (schema v10 `run_schedules`; the daemon's schedule scheduler fires
//! them). Reads run on a fresh read-only connection; mutations ride the
//! daemon's single-writer channel.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_lake::ScheduleRow;

use crate::auth::Principal;
use crate::sql_util::{internal, now_ns, with_reader};
use crate::ApiState;

/// Fire-interval bounds: below 60s the 30s scheduler tick cannot keep up
/// meaningfully; above 30 days is almost certainly a typo.
const MIN_INTERVAL_S: i64 = 60;
const MAX_INTERVAL_S: i64 = 30 * 86_400;

fn bad_request(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response()
}

fn not_found(msg: &str) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"error": msg}))).into_response()
}

fn unavailable(msg: &str) -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": msg}))).into_response()
}

fn forbidden(msg: String) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({"error": msg}))).into_response()
}

/// One schedule as JSON, with the registry name joined for display.
fn schedule_json(s: &ScheduleRow, definition_name: Option<&str>) -> Value {
    json!({
        "id": s.id,
        "name": s.name,
        "registry_id": s.registry_id,
        "definition_name": definition_name,
        "interval_s": s.interval_s,
        "vars_json": s.vars_json,
        "env": s.env,
        "target": s.target,
        "enabled": s.enabled,
        "next_run_at_ns": s.next_run_at_ns,
        "last_run_at_ns": s.last_run_at_ns,
        "last_run_id": s.last_run_id,
        "created_by": s.created_by,
        "created_at_ns": s.created_at_ns,
    })
}

/// Fetch one schedule by id, or a 404 response.
async fn schedule_or_404(state: &ApiState, id: &str) -> Result<ScheduleRow, Response> {
    if id.chars().count() > 100 {
        return Err(bad_request("schedule id too long".into()));
    }
    let lookup = id.to_string();
    let found = with_reader(&state.db_path, move |reader| {
        Ok(reader
            .list_schedules()
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|s| s.id == lookup))
    })
    .await?;
    found.ok_or_else(|| not_found("unknown schedule"))
}

/// `GET /api/schedules` — every schedule with its definition name, ordered
/// by name.
pub async fn list(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let rows = with_reader(&state.db_path, |reader| {
        let names: HashMap<String, String> = reader
            .registry_list(500)
            .map_err(|e| e.to_string())?
            .iter()
            .filter_map(|d| {
                Some((
                    d["id"].as_str()?.to_string(),
                    d["name"].as_str()?.to_string(),
                ))
            })
            .collect();
        let mut out = Vec::new();
        for s in reader.list_schedules().map_err(|e| e.to_string())? {
            out.push(schedule_json(
                &s,
                names.get(&s.registry_id).map(String::as_str),
            ));
        }
        Ok(out)
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "schedules": rows})))
}

/// JSON body for `POST /api/schedules`.
#[derive(Debug, Deserialize)]
pub struct CreateScheduleRequest {
    name: String,
    registry_id: String,
    interval_s: i64,
    #[serde(default)]
    vars: HashMap<String, String>,
    #[serde(default = "default_env")]
    env: String,
    #[serde(default)]
    target: Option<String>,
}

fn default_env() -> String {
    "dev".into()
}

/// `POST /api/schedules` — create an enabled interval schedule. Validates
/// the interval bounds, that the registry id resolves, and that the
/// definition resolves with the supplied variables (the same pipeline the
/// scheduler fires, so a bad schedule fails fast with 400 instead of
/// erroring every tick). The first fire is one interval from creation.
/// A scoped principal may only schedule into its own environments: any
/// other `env` is a 403.
pub async fn create(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<CreateScheduleRequest>,
) -> Result<(StatusCode, Json<Value>), Response> {
    if !principal.env_allowed(&req.env) {
        return Err(forbidden(format!(
            "environment {:?} is outside the principal's scopes",
            req.env
        )));
    }
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(bad_request("name must not be empty".into()));
    }
    if name.chars().count() > 100 {
        return Err(bad_request("name too long (max 100 chars)".into()));
    }
    if !(MIN_INTERVAL_S..=MAX_INTERVAL_S).contains(&req.interval_s) {
        return Err(bad_request(format!(
            "interval_s must be between {MIN_INTERVAL_S} and {MAX_INTERVAL_S} seconds"
        )));
    }
    let registry_id = req.registry_id.clone();
    let def = with_reader(&state.db_path, move |reader| {
        reader
            .registry_definition(&registry_id)
            .map_err(|e| e.to_string())
    })
    .await?
    .ok_or_else(|| not_found("unknown registry id"))?;
    if let Err(e) = tumult_ingest::prepare_run(&def.definition_toon, &req.vars) {
        return Err(bad_request(format!(
            "definition does not resolve with these parameters: {e}"
        )));
    }

    let Some(ingest) = state.ingest_handle() else {
        return Err(unavailable(
            "schedule storage is not wired (no ingest handle)",
        ));
    };
    let vars_json = if req.vars.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&req.vars).unwrap_or_default())
    };
    let now = now_ns();
    let row = ScheduleRow {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        registry_id: def.id,
        interval_s: req.interval_s,
        vars_json,
        env: req.env,
        target: req.target,
        enabled: true,
        next_run_at_ns: now + req.interval_s * 1_000_000_000,
        last_run_at_ns: None,
        last_run_id: None,
        created_by: principal.actor(),
        created_at_ns: now,
    };
    let row2 = row.clone();
    ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            writer.create_schedule(&row2).map_err(|e| e.to_string())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok((
        StatusCode::CREATED,
        Json(schedule_json(&row, Some(&def.name))),
    ))
}

/// JSON body for `POST /api/schedules/{id}/enable`.
#[derive(Debug, Deserialize)]
pub struct EnableScheduleRequest {
    enabled: bool,
}

/// `POST /api/schedules/{id}/enable {enabled}` — flip a schedule on or off.
pub async fn set_enabled(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(req): Json<EnableScheduleRequest>,
) -> Result<Json<Value>, Response> {
    schedule_or_404(&state, &id).await?;
    let Some(ingest) = state.ingest_handle() else {
        return Err(unavailable(
            "schedule storage is not wired (no ingest handle)",
        ));
    };
    let enabled = req.enabled;
    ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            writer
                .set_schedule_enabled(&id, enabled)
                .map_err(|e| e.to_string())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok(Json(json!({"ok": true})))
}

/// `POST /api/schedules/{id}/delete` — remove a schedule. Runs it already
/// fired are untouched.
pub async fn delete(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    schedule_or_404(&state, &id).await?;
    let Some(ingest) = state.ingest_handle() else {
        return Err(unavailable(
            "schedule storage is not wired (no ingest handle)",
        ));
    };
    ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            writer.delete_schedule(&id).map_err(|e| e.to_string())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok(Json(json!({"ok": true})))
}
