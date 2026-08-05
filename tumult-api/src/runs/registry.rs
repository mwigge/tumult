//! Registry reads and definition registration (`GET /api/registry*`,
//! `POST /api/runs/validate`).

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::error::bad_request;
use crate::sql_util::with_reader;
use crate::ApiState;

/// JSON body: the experiment TOON plus optional template variables.
#[derive(Debug, Deserialize)]
pub struct ValidateRequest {
    toon: String,
    #[serde(default)]
    vars: HashMap<String, String>,
}

/// `GET /api/registry` — registered definitions (metadata only), newest
/// first: the UI's registry picker.
pub async fn registry_list(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let rows = with_reader(&state.db_path, |reader| {
        reader.registry_list(500).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "definitions": rows})))
}

/// `GET /api/registry/{id}` — one definition including the `.toon` source
/// (the UI parses `${var}` placeholders from it for the parameter form).
pub async fn registry_detail(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    let def = super::registry_or_404(&state, &id).await?;
    Ok(Json(json!({"definition": def})))
}

/// `POST /api/runs/validate` — run the full parse/resolve/validate pipeline
/// and register the definition (content-hash dedup) so it can be dry-run or
/// enqueued by `registry_id`.
pub async fn validate(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<ValidateRequest>,
) -> Result<Json<Value>, Response> {
    if req.toon.chars().count() > 256_000 {
        return Err(bad_request("definition too large (max 256k chars)"));
    }
    let (experiment, _env) = match tumult_ingest::prepare_run(&req.toon, &req.vars) {
        Ok(prepared) => prepared,
        Err(e) => {
            return Ok(Json(json!({"valid": false, "error": e})));
        }
    };

    // Register through the single-writer channel (content-hash dedup).
    // The authenticated principal when auth is enabled; "api" while open.
    let registration = crate::registry::register_definition(
        &state,
        &req.toon,
        &experiment.title,
        principal.actor().or_else(|| Some("api".into())),
        /* gameday */ false,
        "run registration is not wired (no ingest handle)",
    )
    .await?;
    Ok(Json(json!({
        "valid": true,
        "registry_id": registration.id,
        "name": registration.name,
        "registered": registration.registered,
    })))
}
