//! Admin: `/api/users*` (list, create, role, disable, scopes, password).

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_auth::Role;
use tumult_lake::UserRow;

use crate::sql_util::{internal, now_ns, with_reader};
use crate::ApiState;

use super::middleware::{bad_request, exec_auth_write, not_found};
use super::Principal;

// ---------------------------------------------------------------------------
// Admin: /api/users* + /api/tokens*

/// `GET /api/users` — every user (never the password hash) with env scopes.
pub async fn list_users(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let users = with_reader(&state.db_path, |reader| {
        let users = reader.list_users().map_err(|e| e.to_string())?;
        let mut out = Vec::with_capacity(users.len());
        for u in users {
            let scopes = reader.user_env_scopes(&u.id).map_err(|e| e.to_string())?;
            out.push(json!({
                "id": u.id,
                "username": u.username,
                "role": u.role,
                "must_change": u.must_change,
                "disabled": u.disabled,
                "created_at_ns": u.created_at_ns,
                "env_scopes": scopes,
            }));
        }
        Ok(out)
    })
    .await?;
    Ok(Json(json!({"users": users})))
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    username: String,
    password: Option<String>,
    role: String,
    env_scopes: Option<Vec<String>>,
}

/// `POST /api/users` — create a user with `must_change = true` (always,
/// even when a password was supplied). Without a password, a one-time
/// bootstrap password is generated and returned exactly once.
pub async fn create_user(
    State(state): State<ApiState>,
    Json(req): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<Value>), Response> {
    let username = req.username.trim().to_string();
    if username.is_empty() {
        return Err(bad_request("username must not be empty".into()));
    }
    let Some(role) = Role::parse(&req.role) else {
        return Err(bad_request(format!(
            "unknown role {:?}; expected viewer|operator|approver|admin",
            req.role
        )));
    };
    let lookup = username.clone();
    let existing = with_reader(&state.db_path, move |reader| {
        reader.user_by_username(&lookup).map_err(|e| e.to_string())
    })
    .await?;
    if existing.is_some() {
        return Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "username exists"})),
        )
            .into_response());
    }
    let (password, one_time) = match req.password.filter(|p| !p.is_empty()) {
        Some(p) => (p, None),
        None => {
            let p = tumult_auth::new_password();
            (p.clone(), Some(p))
        }
    };
    let hash = tokio::task::spawn_blocking(move || tumult_auth::hash_password(&password))
        .await
        .map_err(|e| internal(format!("hash task failed: {e}")))?
        .map_err(|e| internal(e.to_string()))?;
    let row = UserRow {
        id: uuid::Uuid::new_v4().to_string(),
        username: username.clone(),
        password_hash: hash,
        role: role.as_str().to_string(),
        must_change: true,
        disabled: false,
        created_at_ns: now_ns(),
    };
    let scopes = req.env_scopes.clone();
    let id = row.id.clone();
    exec_auth_write(&state, move |w| {
        w.create_user(&row).map_err(|e| e.to_string())?;
        if let Some(scopes) = &scopes {
            w.set_user_env_scopes(&row.id, scopes)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    })
    .await?;
    let mut body = json!({
        "id": id,
        "username": username,
        "role": role.as_str(),
        "must_change": true,
    });
    if let Some(p) = one_time {
        body["one_time_password"] = json!(p);
    }
    Ok((StatusCode::CREATED, Json(body)))
}

#[derive(Debug, Deserialize)]
pub struct SetRoleRequest {
    role: String,
}

/// `POST /api/users/{id}/role {role}` — change a user's role.
pub async fn set_role(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(req): Json<SetRoleRequest>,
) -> Result<Json<Value>, Response> {
    let Some(role) = Role::parse(&req.role) else {
        return Err(bad_request(format!(
            "unknown role {:?}; expected viewer|operator|approver|admin",
            req.role
        )));
    };
    user_or_404(&state, &id).await?;
    exec_auth_write(&state, move |w| {
        w.set_user_role(&id, role.as_str())
            .map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Debug, Deserialize)]
pub struct SetDisabledRequest {
    disabled: bool,
}

/// `POST /api/users/{id}/disable {disabled}` — disable/re-enable a user.
/// A principal cannot disable itself.
pub async fn set_disabled(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<SetDisabledRequest>,
) -> Result<Json<Value>, Response> {
    if !principal.synthetic && principal.user_id == id {
        return Err(bad_request("cannot disable yourself".into()));
    }
    user_or_404(&state, &id).await?;
    let disabled = req.disabled;
    exec_auth_write(&state, move |w| {
        w.set_user_disabled(&id, disabled)
            .map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Debug, Deserialize)]
pub struct SetScopesRequest {
    environments: Vec<String>,
}

/// `POST /api/users/{id}/scopes {environments}` — replace a user's
/// environment scopes atomically; an empty list means every environment.
pub async fn set_scopes(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(req): Json<SetScopesRequest>,
) -> Result<Json<Value>, Response> {
    user_or_404(&state, &id).await?;
    let envs = req.environments.clone();
    exec_auth_write(&state, move |w| {
        w.set_user_env_scopes(&id, &envs).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Debug, Deserialize)]
pub struct ResetPasswordRequest {
    password: String,
}

/// `POST /api/users/{id}/password {password}` — admin-driven reset: set a
/// supplied one-time password and force `must_change` at next login (the
/// recovery path for a user who can no longer authenticate; unlike
/// `/api/auth/change-password`, which is self-service and clears the flag).
pub async fn reset_password(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(req): Json<ResetPasswordRequest>,
) -> Result<Json<Value>, Response> {
    if req.password.chars().count() < 12 {
        return Err(bad_request(
            "password must be at least 12 characters".into(),
        ));
    }
    user_or_404(&state, &id).await?;
    let hash = tokio::task::spawn_blocking(move || tumult_auth::hash_password(&req.password))
        .await
        .map_err(|e| internal(format!("hash task failed: {e}")))?
        .map_err(|e| internal(e.to_string()))?;
    exec_auth_write(&state, move |w| {
        w.reset_user_password(&id, &hash).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"ok": true, "must_change": true})))
}

/// 404 unless the user id exists.
pub(crate) async fn user_or_404(state: &ApiState, id: &str) -> Result<UserRow, Response> {
    let lookup = id.to_string();
    let user = with_reader(&state.db_path, move |reader| {
        reader.user_by_id(&lookup).map_err(|e| e.to_string())
    })
    .await?;
    user.ok_or_else(|| not_found("unknown user"))
}
