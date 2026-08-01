//! `POST /api/auth/login` + `/logout` + `/change-password`, `GET /api/me`.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, State};
use axum::http::header::SET_COOKIE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_auth::Role;
use tumult_lake::SessionRow;

use crate::sql_util::{internal, now_ns, with_reader};
use crate::ApiState;

use super::middleware::{
    bad_request, exec_auth_write, session_cookie, session_set_cookie, too_many_requests,
    unauthorized, unavailable,
};
use super::rate_limit::login_limiter;
use super::{Principal, SESSION_TTL_NS};

// ---------------------------------------------------------------------------
// POST /api/auth/login + /logout + /change-password, GET /api/me

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

/// `POST /api/auth/login {username, password}` — verify against the stored
/// argon2id hash (a dummy hash for unknown usernames, so both cost the same
/// ~50 ms), then issue a 12 h session cookie. Every failure is the same
/// generic 401: no user enumeration. Failed attempts are rate-limited per
/// `ip|username` (429 with a generic body once the bucket is empty) and
/// logged for the audit trail — never the password.
pub async fn login(
    State(state): State<ApiState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    Json(req): Json<LoginRequest>,
) -> Result<Response, Response> {
    let Some(_) = state.ingest_handle() else {
        return Err(unavailable("auth writes are not wired (no ingest handle)"));
    };
    let username = req.username.trim().to_string();
    // Best available client identity for the throttle key: the peer address
    // when the server wires `ConnectInfo`, else a shared "unknown" bucket
    // (per-username throttling still applies).
    let client = connect_info.map_or_else(
        || "unknown".to_string(),
        |Extension(ConnectInfo(addr))| addr.ip().to_string(),
    );
    let key = format!("{client}|{username}");
    if login_limiter().throttled(&key) {
        tracing::warn!(
            username = %username,
            client = %client,
            "login throttled: too many failed attempts"
        );
        return Err(too_many_requests("too many attempts; slow down"));
    }
    let lookup = username.clone();
    let user = with_reader(&state.db_path, move |reader| {
        reader.user_by_username(&lookup).map_err(|e| e.to_string())
    })
    .await?;
    // Always pay the argon2 cost, user found or not (timing equalization).
    let hash = user.as_ref().map_or_else(
        || tumult_auth::dummy_password_hash().to_string(),
        |u| u.password_hash.clone(),
    );
    let password = req.password.clone();
    let ok = tokio::task::spawn_blocking(move || tumult_auth::verify_password(&hash, &password))
        .await
        .map_err(|e| internal(format!("verify task failed: {e}")))?;
    let Some(user) = user.filter(|u| ok && !u.disabled) else {
        login_limiter().penalize(&key);
        tracing::warn!(
            username = %username,
            client = %client,
            "login failed: invalid credentials"
        );
        return Err(unauthorized("invalid credentials"));
    };
    let Some(role) = Role::parse(&user.role) else {
        return Err(unauthorized("invalid credentials"));
    };
    login_limiter().reset(&key);

    let session_id = tumult_auth::new_session_id();
    let now = now_ns();
    let row = SessionRow {
        id_hash: tumult_auth::sha256_hex(&session_id),
        user_id: user.id.clone(),
        created_at_ns: now,
        expires_at_ns: now + SESSION_TTL_NS,
    };
    exec_auth_write(&state, move |w| {
        w.create_session(&row).map_err(|e| e.to_string())
    })
    .await?;

    let cookie = session_set_cookie(
        &session_id,
        SESSION_TTL_NS / 1_000_000_000,
        state.secure_cookies(),
    );
    Ok((
        StatusCode::OK,
        [(SET_COOKIE, cookie)],
        Json(json!({
            "username": user.username,
            "role": role.as_str(),
            "must_change": user.must_change,
        })),
    )
        .into_response())
}

/// `POST /api/auth/logout` — drop the session (best-effort) and expire the
/// cookie; always 200.
pub async fn logout(State(state): State<ApiState>, headers: HeaderMap) -> Response {
    if let Some(id) = session_cookie(&headers) {
        if state.ingest_handle().is_some() {
            let hash = tumult_auth::sha256_hex(&id);
            let _ = exec_auth_write(&state, move |w| {
                w.delete_session(&hash).map_err(|e| e.to_string())
            })
            .await;
        }
    }
    (
        [(
            SET_COOKIE,
            session_set_cookie("", 0, state.secure_cookies()),
        )],
        Json(json!({"ok": true})),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

/// `POST /api/auth/change-password {current_password, new_password}` — the
/// authenticated user replaces their own password (clears `must_change`).
pub async fn change_password(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<ChangePasswordRequest>,
) -> Result<Json<Value>, Response> {
    if principal.synthetic {
        return Err(bad_request("authentication is not enabled".into()));
    }
    if req.new_password.chars().count() < 12 {
        return Err(bad_request(
            "new_password must be at least 12 characters".into(),
        ));
    }
    let user_id = principal.user_id.clone();
    let lookup = user_id.clone();
    let user = with_reader(&state.db_path, move |reader| {
        reader.user_by_id(&lookup).map_err(|e| e.to_string())
    })
    .await?
    .ok_or_else(|| unauthorized("invalid credentials"))?;
    let current = req.current_password.clone();
    let hash = user.password_hash.clone();
    let ok = tokio::task::spawn_blocking(move || tumult_auth::verify_password(&hash, &current))
        .await
        .map_err(|e| internal(format!("verify task failed: {e}")))?;
    if !ok {
        return Err(unauthorized("invalid credentials"));
    }
    let new_hash =
        tokio::task::spawn_blocking(move || tumult_auth::hash_password(&req.new_password))
            .await
            .map_err(|e| internal(format!("hash task failed: {e}")))?
            .map_err(|e| internal(e.to_string()))?;
    exec_auth_write(&state, move |w| {
        w.set_user_password(&user_id, &new_hash)
            .map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"changed": true})))
}

/// `GET /api/me` — always 200: whether auth is required, and the caller's
/// identity when a valid credential rode along.
pub async fn me(principal: Option<Extension<Principal>>) -> Json<Value> {
    match principal {
        // The synthetic principal only exists while auth is open.
        None => Json(json!({"auth_required": true, "authenticated": false})),
        Some(Extension(p)) if p.synthetic => {
            Json(json!({"auth_required": false, "authenticated": false}))
        }
        Some(Extension(p)) => Json(json!({
            "auth_required": true,
            "authenticated": true,
            "username": p.username,
            "role": p.role.as_str(),
            "must_change": p.must_change,
            "env_scopes": p.env_scopes,
        })),
    }
}
