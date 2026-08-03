//! Admin: `/api/tokens*` (create, revoke).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_lake::TokenRow;

use crate::sql_util::{now_ns, with_reader};
use crate::ApiState;

use super::middleware::{bad_request, exec_auth_write};
use super::users::user_or_404;
use super::Principal;

/// `GET /api/tokens` — every token (newest first, including revoked) with
/// the owner's username; never the token hash. Admin-only via the route
/// table, like the rest of `/api/tokens*`.
pub async fn list_tokens(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let tokens = with_reader(&state.db_path, |reader| {
        let usernames: std::collections::HashMap<String, String> = reader
            .list_users()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|u| (u.id, u.username))
            .collect();
        let mut out = Vec::new();
        for t in reader.list_tokens().map_err(|e| e.to_string())? {
            out.push(json!({
                "id": t.id,
                "user_id": t.user_id,
                "username": usernames.get(&t.user_id),
                "name": t.name,
                "created_at_ns": t.created_at_ns,
                "last_used_at_ns": t.last_used_at_ns,
                "revoked": t.revoked,
                "expires_at_ns": t.expires_at_ns,
            }));
        }
        Ok(out)
    })
    .await?;
    Ok(Json(json!({"tokens": tokens})))
}

#[derive(Debug, Deserialize)]
pub struct CreateTokenRequest {
    name: String,
    user_id: Option<String>,
    /// Optional absolute expiry (ns since epoch); omitted = never expires.
    expires_at_ns: Option<i64>,
}

/// `POST /api/tokens {name, user_id?, expires_at_ns?}` — mint a `kro_` API
/// token (default owner: the caller, default expiry: never). The plaintext
/// token appears only in this response; the store keeps its sha256.
pub async fn create_token(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<CreateTokenRequest>,
) -> Result<(StatusCode, Json<Value>), Response> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(bad_request("name must not be empty".into()));
    }
    let now = now_ns();
    if req.expires_at_ns.is_some_and(|t| t <= now) {
        return Err(bad_request("expires_at_ns must be in the future".into()));
    }
    let user_id = req
        .user_id
        .clone()
        .unwrap_or_else(|| principal.user_id.clone());
    user_or_404(&state, &user_id).await?;
    let token = tumult_auth::new_token();
    let row = TokenRow {
        id: uuid::Uuid::new_v4().to_string(),
        user_id,
        name,
        token_hash: tumult_auth::sha256_hex(&token),
        created_at_ns: now,
        last_used_at_ns: None,
        revoked: false,
        expires_at_ns: req.expires_at_ns,
    };
    let id = row.id.clone();
    let expires_at_ns = row.expires_at_ns;
    exec_auth_write(&state, move |w| {
        w.create_token(&row).map_err(|e| e.to_string())
    })
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"id": id, "token": token, "expires_at_ns": expires_at_ns})),
    ))
}

/// `POST /api/tokens/{id}/revoke` — revoke a token by its record id.
pub async fn revoke_token(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    exec_auth_write(&state, move |w| {
        w.revoke_token(&id).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}
