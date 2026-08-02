//! GameDay endpoints (`/api/gamedays*`) — registration and inspection of
//! GameDay campaigns (schema v12 `run_registry.kind = 'gameday'`).
//!
//! Registration mirrors `POST /api/runs/validate`: each experiment TOON the
//! campaign references is run through the same parse/resolve/validate
//! pipeline and registered (content-hash dedup, `kind` NULL = experiment),
//! then the campaign itself is registered with `kind = 'gameday'`. The
//! gameday's `definition_toon` cell holds a JSON envelope:
//! `{"toon": <campaign toon>, "experiments": [{path, registry_id}]}` — the
//! resolved mapping the campaign runner (separate change) dispatches from.

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_lake::RegisteredDefinition;

use crate::auth::Principal;
use crate::sql_util::{internal, now_ns, with_reader};
use crate::ApiState;

fn bad_request(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response()
}

fn not_found(msg: &str) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"error": msg}))).into_response()
}

fn unavailable(msg: &str) -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": msg}))).into_response()
}

/// SHA-256 hex of a definition — the dedup key; registry ids derive from it
/// (`reg-<first 12 hex>`), same rule as experiment registration.
fn content_hash(text: &str) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// JSON body for `POST /api/gamedays/validate`: the campaign TOON plus its
/// referenced experiment TOONs keyed by the exact `path` strings the
/// campaign uses.
#[derive(Debug, Deserialize)]
pub struct ValidateGameDayRequest {
    toon: String,
    #[serde(default)]
    experiments: HashMap<String, String>,
}

/// The resolved experiment mapping inside a gameday's JSON envelope.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GameDayStep {
    path: String,
    registry_id: String,
}

/// `POST /api/gamedays/validate` — parse the campaign, validate and register
/// every referenced experiment (deduped), and register the campaign itself.
/// Returns the gameday registry id and the ordered experiment steps.
pub async fn validate(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<ValidateGameDayRequest>,
) -> Result<Json<Value>, Response> {
    if req.toon.chars().count() > 256_000 {
        return Err(bad_request("gameday too large (max 256k chars)".into()));
    }
    let gameday: tumult_core::types::GameDay = toon_format::decode_default(&req.toon)
        .map_err(|e| bad_request(format!("gameday does not parse: {e}")))?;
    if gameday.experiments.is_empty() {
        return Err(bad_request("gameday has no experiments".into()));
    }
    // Every referenced path must come with its TOON; validate each through
    // the normal run pipeline before anything is registered.
    for step in &gameday.experiments {
        let path = step.path.to_string_lossy().into_owned();
        let Some(toon) = req.experiments.get(&path) else {
            return Err(bad_request(format!(
                "experiment {path:?} referenced by the gameday was not supplied"
            )));
        };
        if let Err(e) = tumult_ingest::prepare_run(toon, &HashMap::new()) {
            return Err(bad_request(format!("experiment {path:?}: {e}")));
        }
    }

    let Some(ingest) = state.ingest_handle() else {
        return Err(unavailable(
            "gameday registration is not wired (no ingest handle)",
        ));
    };

    // Register each experiment (content-hash dedup, kind NULL = experiment).
    let mut steps = Vec::with_capacity(gameday.experiments.len());
    for step in &gameday.experiments {
        let path = step.path.to_string_lossy().into_owned();
        let toon = req.experiments[&path].clone();
        let (experiment, _env) =
            tumult_ingest::prepare_run(&toon, &HashMap::new()).map_err(internal)?;
        let id = register_definition(
            &state,
            ingest,
            &toon,
            &experiment.title,
            principal.actor(),
            /* gameday */ false,
        )
        .await?;
        steps.push(GameDayStep {
            path,
            registry_id: id,
        });
    }

    // Register the campaign envelope itself (kind = 'gameday'). The
    // scoring config rides along so the supervisor needs no TOON parse.
    let envelope =
        json!({"toon": req.toon, "experiments": steps, "scoring": gameday.scoring}).to_string();
    let gameday_id = register_definition(
        &state,
        ingest,
        &envelope,
        &gameday.title,
        principal.actor(),
        true,
    )
    .await?;
    Ok(Json(json!({
        "valid": true,
        "gameday_registry_id": gameday_id,
        "experiments": steps,
    })))
}

/// Register one definition by content hash (dedup: an identical TOON lands
/// on the existing row) and return its registry id.
async fn register_definition(
    state: &ApiState,
    ingest: &tumult_ingest::IngestWriter,
    text: &str,
    name: &str,
    actor: Option<String>,
    gameday: bool,
) -> Result<String, Response> {
    let hash = content_hash(text);
    let lookup = hash.clone();
    let existing = with_reader(&state.db_path, move |reader| {
        reader.registry_by_hash(&lookup).map_err(|e| e.to_string())
    })
    .await?;
    if let Some(def) = existing {
        return Ok(def.id);
    }
    let def = RegisteredDefinition {
        id: format!("reg-{}", &hash[..12]),
        name: name.to_string(),
        definition_toon: text.to_string(),
        content_hash: hash,
        registered_at_ns: now_ns(),
        registered_by: actor,
    };
    let id = def.id.clone();
    ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            if gameday {
                writer
                    .register_gameday_definition(&def)
                    .map_err(|e| e.to_string())
            } else {
                writer.register_definition(&def).map_err(|e| e.to_string())
            }
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok(id)
}

/// `GET /api/gamedays` — registered campaigns (metadata only), newest first.
pub async fn list(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let rows = with_reader(&state.db_path, |reader| {
        reader
            .query_json_rows(
                "SELECT id, name, content_hash, registered_at_ns, registered_by \
                 FROM run_registry WHERE kind = 'gameday' \
                 ORDER BY registered_at_ns DESC LIMIT 500",
            )
            .map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "gamedays": rows})))
}

/// Fetch one gameday registry row, or a 404 response.
async fn gameday_or_404(state: &ApiState, id: &str) -> Result<Value, Response> {
    if id.chars().count() > 100 {
        return Err(bad_request("gameday id too long".into()));
    }
    let lookup = id.to_string();
    let rows = with_reader(&state.db_path, move |reader| {
        reader
            .query_json_rows(&format!(
                "SELECT * FROM run_registry WHERE id = '{}' AND kind = 'gameday'",
                lookup.replace('\'', "''")
            ))
            .map_err(|e| e.to_string())
    })
    .await?;
    rows.into_iter()
        .next()
        .ok_or_else(|| not_found("unknown gameday"))
}

/// `GET /api/gamedays/{id}` — the parsed campaign plan: title, description,
/// tags, regulatory mapping, scoring thresholds, and the ordered experiment
/// steps with their registry ids and names.
pub async fn detail(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    let row = gameday_or_404(&state, &id).await?;
    let envelope: Value = serde_json::from_str(row["definition_toon"].as_str().unwrap_or("{}"))
        .map_err(|e| internal(e.to_string()))?;
    let gameday: tumult_core::types::GameDay =
        toon_format::decode_default(envelope["toon"].as_str().unwrap_or_default())
            .map_err(|e| internal(format!("stored gameday failed to re-parse: {e}")))?;
    // Resolve step names from the registry (the campaign TOON only has paths).
    let steps = envelope["experiments"].clone();
    let step_ids: Vec<String> = steps
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| s["registry_id"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let names = with_reader(&state.db_path, move |reader| {
        let mut out = HashMap::new();
        for id in step_ids {
            if let Ok(Some(def)) = reader.registry_definition(&id) {
                out.insert(id, def.name);
            }
        }
        Ok(out)
    })
    .await?;
    let experiments: Vec<Value> = gameday
        .experiments
        .iter()
        .zip(steps.as_array().into_iter().flatten())
        .map(|(step, resolved)| {
            let registry_id = resolved["registry_id"].as_str().unwrap_or_default();
            json!({
                "path": step.path,
                "compliance_maps": step.compliance_maps,
                "registry_id": registry_id,
                "name": names.get(registry_id),
            })
        })
        .collect();
    Ok(Json(json!({
        "id": row["id"],
        "title": gameday.title,
        "description": gameday.description,
        "tags": gameday.tags,
        "regulatory": gameday.regulatory,
        "scoring": gameday.scoring,
        "experiments": experiments,
        "registered_at_ns": row["registered_at_ns"],
        "registered_by": row["registered_by"],
    })))
}

/// JSON body for `POST /api/gamedays/{id}/runs`.
#[derive(Debug, Deserialize)]
pub struct CreateCampaignRequest {
    #[serde(default = "default_env")]
    env: String,
}

fn default_env() -> String {
    "dev".into()
}

/// `POST /api/gamedays/{id}/runs {env?}` — start a campaign: a parent run
/// the daemon's gameday supervisor advances through the campaign's
/// experiments as sequential child runs (each with the campaign's env, so
/// tier classification and approvals apply per step). 409 while another
/// campaign of the same gameday is active.
pub async fn start_campaign(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<CreateCampaignRequest>,
) -> Result<(StatusCode, Json<Value>), Response> {
    let row = gameday_or_404(&state, &id).await?;
    let envelope: Value = serde_json::from_str(row["definition_toon"].as_str().unwrap_or("{}"))
        .map_err(|e| internal(e.to_string()))?;
    let steps = envelope["experiments"].as_array().map_or(0, Vec::len);
    let lookup = id.clone();
    let active = with_reader(&state.db_path, move |reader| {
        reader
            .query_json_rows(&format!(
                "SELECT r.id FROM runs r \
                 WHERE r.registry_id = '{}' AND r.gameday_id IS NULL \
                   AND r.state IN ('queued', 'running') LIMIT 1",
                lookup.replace('\'', "''")
            ))
            .map_err(|e| e.to_string())
    })
    .await?;
    if !active.is_empty() {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "a campaign is already active for this gameday"})),
        )
            .into_response());
    }

    let Some(ingest) = state.ingest_handle() else {
        return Err(unavailable("run creation is not wired (no ingest handle)"));
    };
    let run_id = uuid::Uuid::new_v4().to_string();
    let new_run = tumult_lake::NewRun {
        id: run_id.clone(),
        registry_id: id,
        params_json: Some(json!({"env": req.env}).to_string()),
        queued_at_ns: now_ns(),
        actor: principal.actor(),
    };
    ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            writer.insert_run(&new_run).map_err(|e| e.to_string())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    Ok((
        StatusCode::ACCEPTED,
        Json(json!({"run_id": run_id, "state": "queued", "steps": steps})),
    ))
}
