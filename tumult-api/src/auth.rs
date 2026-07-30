//! Authentication, RBAC and per-user environment scoping.
//!
//! Auth is **enabled iff the store has any real users** (checked per
//! request, so bootstrapping the first user flips it on without a restart).
//! The `legacy` backfill identity seeded by the v6 migration does not count:
//! a store holding only that row is still open. While open, the middleware
//! injects a synthetic admin [`Principal`] and the whole API behaves exactly
//! as before auth existed.
//!
//! Credentials resolve in order: an `Authorization: Bearer kro_…` API token
//! (stored as its sha256), then the `kro_session` cookie (also stored as its
//! sha256; sessions live 12 h). Failure is always the same generic 401, and
//! authorization then maps `(method, path)` through [`ROUTE_TABLE`] — any
//! route missing from the table fails closed at [`Role::Admin`].
//!
//! All mutations ride the daemon's single-writer channel via
//! [`tumult_ingest::Batch::Exec`], like every other write endpoint; reads
//! run on a fresh read-only connection.

use std::sync::{Arc, Mutex};

use axum::extract::{Path, Request, State};
use axum::http::header::{AUTHORIZATION, COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_auth::Role;
use tumult_ingest::Batch;
use tumult_lake::{Reader, SessionRow, Store, TokenRow, UserRow, Writer};

use crate::{internal, now_ns, with_reader, ApiState};

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "kro_session";
/// Session lifetime: 12 hours, in nanoseconds (matches `Max-Age=43200`).
pub const SESSION_TTL_NS: i64 = 12 * 3600 * 1_000_000_000;

/// The authenticated identity behind a request, inserted into the request
/// extensions by [`auth_middleware`] and extracted by handlers via
/// `Extension<Principal>`.
#[derive(Clone, Debug)]
pub struct Principal {
    pub user_id: String,
    pub username: String,
    pub role: Role,
    /// Allowed environments; empty means every environment (also the case
    /// for the synthetic open-auth principal).
    pub env_scopes: Vec<String>,
    pub must_change: bool,
    /// `true` for the synthetic admin injected while auth is open (zero
    /// users): identity-bearing request fields (`entered_by` and friends)
    /// fall back to the request body, exactly as before auth existed. `false`
    /// for a real authenticated user, whose username then *overrides* those
    /// fields.
    pub synthetic: bool,
}

impl Principal {
    /// The open-auth stand-in: admin, all environments, username
    /// "anonymous".
    pub fn synthetic() -> Self {
        Self {
            user_id: String::new(),
            username: "anonymous".into(),
            role: Role::Admin,
            env_scopes: Vec::new(),
            must_change: false,
            synthetic: true,
        }
    }

    /// The username to record as actor, or `None` for the synthetic
    /// open-auth principal (system actor, as before auth existed).
    pub fn actor(&self) -> Option<String> {
        (!self.synthetic).then(|| self.username.clone())
    }
}

// ---------------------------------------------------------------------------
// Route table: (method, path template, minimum role). The single source of
// truth for authorization; `{...}` segments match exactly one path segment.
// Anything not listed fails closed at Admin.

pub const ROUTE_TABLE: &[(&str, &str, Role)] = &[
    // Reads: every GET is Viewer, except the user list (Admin).
    ("GET", "/api/overview", Role::Viewer),
    ("GET", "/api/timeseries", Role::Viewer),
    ("GET", "/api/experiments", Role::Viewer),
    ("GET", "/api/experiments/windows", Role::Viewer),
    ("GET", "/api/experiments/{id}", Role::Viewer),
    ("GET", "/api/dimensions", Role::Viewer),
    ("GET", "/api/metrics", Role::Viewer),
    ("GET", "/api/logs", Role::Viewer),
    ("GET", "/api/logs/volume", Role::Viewer),
    ("GET", "/api/traces", Role::Viewer),
    ("GET", "/api/traces/durations", Role::Viewer),
    ("GET", "/api/traces/{id}", Role::Viewer),
    ("GET", "/api/metrics/catalog", Role::Viewer),
    ("GET", "/api/metrics/query", Role::Viewer),
    ("GET", "/api/topology", Role::Viewer),
    ("GET", "/api/scores", Role::Viewer),
    ("GET", "/api/scores/tree", Role::Viewer),
    ("GET", "/api/manual/experiments", Role::Viewer),
    ("GET", "/api/manual/experiments/{id}", Role::Viewer),
    ("GET", "/api/registry", Role::Viewer),
    ("GET", "/api/registry/{id}", Role::Viewer),
    ("GET", "/api/runs", Role::Viewer),
    ("GET", "/api/runs/{id}", Role::Viewer),
    ("GET", "/api/lake/status", Role::Viewer),
    ("GET", "/api/reports", Role::Viewer),
    ("GET", "/api/reports/v2", Role::Viewer),
    ("GET", "/api/reports/v2/{id}/pdf", Role::Viewer),
    ("GET", "/api/reports/v2/{id}/html", Role::Viewer),
    ("GET", "/api/reports/{name}", Role::Viewer),
    ("GET", "/api/me", Role::Viewer),
    ("GET", "/api/users", Role::Admin),
    // Viewer-level writes (no fault injection, no state change).
    ("POST", "/api/ask", Role::Viewer),
    ("POST", "/api/runs/dry-run", Role::Viewer),
    ("POST", "/api/auth/login", Role::Viewer),
    ("POST", "/api/auth/logout", Role::Viewer),
    ("POST", "/api/auth/change-password", Role::Viewer),
    // Operator: run execution, imports, manual-evidence entry, reports.
    ("POST", "/api/runs", Role::Operator),
    ("POST", "/api/runs/{id}/stop", Role::Operator),
    ("POST", "/api/runs/validate", Role::Operator),
    ("POST", "/api/import/journal", Role::Operator),
    ("POST", "/api/manual/experiments", Role::Operator),
    ("PUT", "/api/manual/experiments/{id}", Role::Operator),
    (
        "POST",
        "/api/manual/experiments/{id}/submit",
        Role::Operator,
    ),
    (
        "POST",
        "/api/manual/experiments/{id}/attachments",
        Role::Operator,
    ),
    ("POST", "/api/manual/import", Role::Operator),
    ("POST", "/api/reports/generate", Role::Operator),
    ("POST", "/api/reports/v2/generate", Role::Operator),
    ("POST", "/api/lake/export", Role::Operator),
    // Approver: manual-evidence review.
    (
        "POST",
        "/api/manual/experiments/{id}/verify",
        Role::Approver,
    ),
    (
        "POST",
        "/api/manual/experiments/{id}/reject",
        Role::Approver,
    ),
    // Approvals: the queue is a read; decisions need the Approver role;
    // break-glass is Admin-only (ADR-012).
    ("GET", "/api/approvals", Role::Viewer),
    ("POST", "/api/runs/{id}/approve", Role::Approver),
    ("POST", "/api/runs/{id}/reject", Role::Approver),
    ("POST", "/api/runs/{id}/break-glass", Role::Admin),
    // Admin: user and token management.
    ("POST", "/api/users", Role::Admin),
    ("POST", "/api/users/{id}/role", Role::Admin),
    ("POST", "/api/users/{id}/disable", Role::Admin),
    ("POST", "/api/users/{id}/scopes", Role::Admin),
    ("POST", "/api/tokens", Role::Admin),
    ("POST", "/api/tokens/{id}/revoke", Role::Admin),
];

/// Whether one path template matches a concrete path: a `{...}` segment
/// matches exactly one (non-empty) segment, literals match verbatim.
fn template_matches(template: &str, path: &str) -> bool {
    let t: Vec<&str> = template.split('/').collect();
    let p: Vec<&str> = path.split('/').collect();
    t.len() == p.len()
        && t.iter().zip(&p).all(|(t, p)| {
            if t.starts_with('{') && t.ends_with('}') {
                !p.is_empty()
            } else {
                t == p
            }
        })
}

/// Number of literal (non-`{...}`) segments — the specificity rank.
fn literal_segments(template: &str) -> usize {
    template
        .split('/')
        .filter(|s| !(s.starts_with('{') && s.ends_with('}')))
        .count()
}

/// The minimum role for `(method, path)`: the most specific matching table
/// entry (most literal segments, so `/api/runs/dry-run` beats
/// `/api/runs/{id}`), or [`Role::Admin`] when nothing matches — fail closed.
pub(crate) fn required_role(method: &str, path: &str) -> Role {
    ROUTE_TABLE
        .iter()
        .filter(|(m, t, _)| *m == method && template_matches(t, path))
        .max_by_key(|(_, t, _)| literal_segments(t))
        .map_or(Role::Admin, |(_, _, r)| *r)
}

// ---------------------------------------------------------------------------
// Middleware

/// 401 JSON response.
fn unauthorized(msg: &str) -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({"error": msg}))).into_response()
}

/// 403 JSON response.
fn forbidden(msg: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({"error": msg}))).into_response()
}

/// 400 JSON response.
fn bad_request(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response()
}

/// 404 JSON response.
fn not_found(msg: &str) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"error": msg}))).into_response()
}

/// 503 JSON response (mutating endpoint without the daemon's writer).
fn unavailable(msg: &str) -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": msg}))).into_response()
}

/// The `kro_session` cookie value, parsed by hand from the `Cookie` header.
fn session_cookie(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        if let Some(v) = part.trim().strip_prefix("kro_session=") {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// The `Authorization: Bearer …` token.
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::to_string)
}

/// The Set-Cookie value for the session cookie; `; Secure` only behind TLS
/// (`ApiState::secure_cookies`).
fn session_set_cookie(value: &str, max_age_s: i64, secure: bool) -> String {
    let mut v =
        format!("{SESSION_COOKIE}={value}; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age_s}");
    if secure {
        v.push_str("; Secure");
    }
    v
}

/// Store-side outcome of one middleware pass.
enum Resolve {
    /// Zero users: auth is open.
    Open,
    /// Valid credential (the token hash to touch, when a bearer token
    /// authenticated).
    Authenticated(Principal, Option<String>),
    /// No/invalid credential, disabled user, or unparseable stored role.
    Rejected,
}

/// Resolve the request's credentials against the store. Runs inside
/// `spawn_blocking` (DuckDB is synchronous).
fn resolve(
    reader: &Reader,
    bearer: Option<&str>,
    cookie: Option<&str>,
    now: i64,
) -> Result<Resolve, String> {
    if !reader.real_users_exist().map_err(|e| e.to_string())? {
        return Ok(Resolve::Open);
    }
    let resolved: Option<(UserRow, Option<String>)> =
        if let Some(token) = bearer.filter(|t| t.starts_with("kro_")) {
            let hash = tumult_auth::sha256_hex(token);
            reader
                .token_with_user(&hash)
                .map_err(|e| e.to_string())?
                .map(|(_, user)| (user, Some(hash)))
        } else if let Some(session_id) = cookie {
            let hash = tumult_auth::sha256_hex(session_id);
            reader
                .session_with_user(&hash, now)
                .map_err(|e| e.to_string())?
                .map(|(_, user)| (user, None))
        } else {
            None
        };
    let Some((user, token_hash)) = resolved else {
        return Ok(Resolve::Rejected);
    };
    // Fail closed on a disabled account or an unreadable stored role.
    if user.disabled {
        return Ok(Resolve::Rejected);
    }
    let Some(role) = Role::parse(&user.role) else {
        return Ok(Resolve::Rejected);
    };
    let env_scopes = reader
        .user_env_scopes(&user.id)
        .map_err(|e| e.to_string())?;
    Ok(Resolve::Authenticated(
        Principal {
            user_id: user.id,
            username: user.username,
            role,
            env_scopes,
            must_change: user.must_change,
            synthetic: false,
        },
        token_hash,
    ))
}

/// Routes a `must_change` principal may still reach (besides login/me,
/// which pass through before this check).
const PASSWORD_CHANGE_EXEMPT: &[&str] = &["/api/auth/logout", "/api/auth/change-password"];

/// Authentication + authorization middleware covering every `/api/*` route.
///
/// Exempt from the credential requirement: `POST /api/auth/login` (it does
/// its own lookup) and `GET /api/me` (it reports `authenticated: false`
/// instead of failing).
pub async fn auth_middleware(
    State(state): State<ApiState>,
    mut req: Request,
    next: Next,
) -> Response {
    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    let is_login = method == "POST" && path == "/api/auth/login";
    let is_me = method == "GET" && path == "/api/me";

    let bearer = bearer_token(req.headers());
    let cookie = session_cookie(req.headers());
    let now = now_ns();
    let db = state.db_path.as_ref().clone();
    let resolved = tokio::task::spawn_blocking(move || {
        let reader = Store::at(&db).read_only().map_err(|e| e.to_string())?;
        resolve(&reader, bearer.as_deref(), cookie.as_deref(), now)
    })
    .await;
    let resolved = match resolved {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return internal(e),
        Err(e) => return internal(format!("auth task failed: {e}")),
    };

    match resolved {
        Resolve::Open => {
            // Zero users: the API behaves exactly as before auth existed.
            req.extensions_mut().insert(Principal::synthetic());
        }
        Resolve::Rejected => {
            if !is_login && !is_me {
                return unauthorized("authentication required");
            }
            // login / me pass through without a principal.
        }
        Resolve::Authenticated(principal, token_hash) => {
            if is_login || is_me {
                // Already-authenticated callers may re-login or inspect
                // themselves; no gates apply to these two routes.
                req.extensions_mut().insert(principal);
                return next.run(req).await;
            }
            // Best-effort token usage stamp (never fails the request).
            if let (Some(hash), Some(ingest)) = (token_hash, state.ingest_handle().cloned()) {
                let _ = ingest
                    .write(Batch::Exec(Box::new(move |writer: &Writer| {
                        let _ = writer.touch_token_last_used(&hash, now);
                        Ok(())
                    })))
                    .await;
            }
            if principal.must_change && !PASSWORD_CHANGE_EXEMPT.contains(&path.as_str()) {
                return forbidden("password_change_required");
            }
            let required = required_role(&method, &path);
            if principal.role < required {
                return forbidden("insufficient role");
            }
            req.extensions_mut().insert(principal);
        }
    }
    next.run(req).await
}

// ---------------------------------------------------------------------------
// Writer-channel helper (the `Batch::Exec` slot pattern of import.rs /
// manual.rs, generalized to the auth mutations).

/// Run one auth mutation on the daemon's single writer; 503 when the API
/// runs without the ingest handle (non-daemon mode), like the other
/// mutating endpoints.
async fn exec_auth_write(
    state: &ApiState,
    f: impl FnOnce(&Writer) -> Result<(), String> + Send + 'static,
) -> Result<(), Response> {
    let Some(ingest) = state.ingest_handle() else {
        return Err(unavailable("auth writes are not wired (no ingest handle)"));
    };
    let slot = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    ingest
        .write(Batch::Exec(Box::new(move |writer: &Writer| {
            *slot2.lock().unwrap_or_else(|e| e.into_inner()) = Some(f(writer));
            Ok(())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    let result = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    match result {
        Some(Ok(())) => Ok(()),
        Some(Err(e)) => Err(internal(e)),
        None => Err(internal("auth write did not run".into())),
    }
}

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
/// generic 401: no user enumeration.
pub async fn login(
    State(state): State<ApiState>,
    Json(req): Json<LoginRequest>,
) -> Result<Response, Response> {
    let Some(_) = state.ingest_handle() else {
        return Err(unavailable("auth writes are not wired (no ingest handle)"));
    };
    let username = req.username.trim().to_string();
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
        return Err(unauthorized("invalid credentials"));
    };
    let Some(role) = Role::parse(&user.role) else {
        return Err(unauthorized("invalid credentials"));
    };

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

/// 404 unless the user id exists.
async fn user_or_404(state: &ApiState, id: &str) -> Result<UserRow, Response> {
    let lookup = id.to_string();
    let user = with_reader(&state.db_path, move |reader| {
        reader.user_by_id(&lookup).map_err(|e| e.to_string())
    })
    .await?;
    user.ok_or_else(|| not_found("unknown user"))
}

#[derive(Debug, Deserialize)]
pub struct CreateTokenRequest {
    name: String,
    user_id: Option<String>,
}

/// `POST /api/tokens {name, user_id?}` — mint a `kro_` API token (default
/// owner: the caller). The plaintext token appears only in this response;
/// the store keeps its sha256.
pub async fn create_token(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<CreateTokenRequest>,
) -> Result<(StatusCode, Json<Value>), Response> {
    let name = req.name.trim().to_string();
    if name.is_empty() {
        return Err(bad_request("name must not be empty".into()));
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
        created_at_ns: now_ns(),
        last_used_at_ns: None,
        revoked: false,
    };
    let id = row.id.clone();
    exec_auth_write(&state, move |w| {
        w.create_token(&row).map_err(|e| e.to_string())
    })
    .await?;
    Ok((StatusCode::CREATED, Json(json!({"id": id, "token": token}))))
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

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_and_validate_beat_the_runs_id_template() {
        // Literal-heavy templates win over `/api/runs/{id}`.
        assert_eq!(required_role("POST", "/api/runs/dry-run"), Role::Viewer);
        assert_eq!(required_role("POST", "/api/runs/validate"), Role::Operator);
        assert_eq!(required_role("GET", "/api/runs/some-id"), Role::Viewer);
        assert_eq!(
            required_role("POST", "/api/runs/some-id/stop"),
            Role::Operator
        );
    }

    #[test]
    fn table_roles_match_the_design() {
        assert_eq!(required_role("GET", "/api/experiments"), Role::Viewer);
        assert_eq!(required_role("GET", "/api/users"), Role::Admin);
        assert_eq!(required_role("POST", "/api/runs"), Role::Operator);
        assert_eq!(
            required_role("POST", "/api/manual/experiments/x/verify"),
            Role::Approver
        );
        assert_eq!(
            required_role("POST", "/api/manual/experiments/x/reject"),
            Role::Approver
        );
        assert_eq!(required_role("POST", "/api/users"), Role::Admin);
        assert_eq!(required_role("POST", "/api/tokens/x/revoke"), Role::Admin);
    }

    #[test]
    fn unmatched_routes_fail_closed_at_admin() {
        assert_eq!(required_role("DELETE", "/api/runs"), Role::Admin);
        assert_eq!(required_role("POST", "/api/runs/some-id"), Role::Admin);
        assert_eq!(required_role("GET", "/api/nope"), Role::Admin);
        assert_eq!(required_role("POST", "/api/nope/nope"), Role::Admin);
    }

    #[test]
    fn template_matcher_segment_semantics() {
        assert!(template_matches("/api/runs/{id}", "/api/runs/abc"));
        assert!(!template_matches("/api/runs/{id}", "/api/runs/abc/stop"));
        assert!(!template_matches("/api/runs/{id}", "/api/runs"));
        assert!(template_matches(
            "/api/manual/experiments/{id}/verify",
            "/api/manual/experiments/x/verify"
        ));
    }

    #[test]
    fn set_cookie_shape() {
        let c = session_set_cookie("abc", 43_200, false);
        assert_eq!(
            c,
            "kro_session=abc; HttpOnly; SameSite=Strict; Path=/; Max-Age=43200"
        );
        assert!(session_set_cookie("abc", 43_200, true).ends_with("; Secure"));
        assert!(session_set_cookie("", 0, false).contains("Max-Age=0"));
    }

    #[test]
    fn cookie_header_parsing() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            "other=1; kro_session=xyz ; theme=dark".parse().unwrap(),
        );
        assert_eq!(session_cookie(&headers).as_deref(), Some("xyz"));
        let empty = HeaderMap::new();
        assert_eq!(session_cookie(&empty), None);
    }
}
