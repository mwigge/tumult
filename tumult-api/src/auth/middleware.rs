//! Credential resolution, the auth middleware and the response/cookie
//! helpers shared by the auth endpoints.

use std::sync::{Arc, Mutex, Once};

use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use tumult_auth::Role;
use tumult_ingest::Batch;
use tumult_lake::{Reader, Store, UserRow, Writer};

use crate::sql_util::{internal, now_ns};
use crate::ApiState;

use super::route_table::required_role;
use super::{Principal, SESSION_COOKIE};

// ---------------------------------------------------------------------------
// Middleware

/// 401 JSON response.
pub(crate) fn unauthorized(msg: &str) -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({"error": msg}))).into_response()
}

/// 403 JSON response.
pub(crate) fn forbidden(msg: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({"error": msg}))).into_response()
}

/// 400 JSON response.
pub(crate) fn bad_request(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response()
}

/// 404 JSON response.
pub(crate) fn not_found(msg: &str) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"error": msg}))).into_response()
}

/// 429 JSON response (login throttling; generic body like the 401s).
pub(crate) fn too_many_requests(msg: &str) -> Response {
    (StatusCode::TOO_MANY_REQUESTS, Json(json!({"error": msg}))).into_response()
}

/// 503 JSON response (mutating endpoint without the daemon's writer).
pub(crate) fn unavailable(msg: &str) -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": msg}))).into_response()
}

/// The `kro_session` cookie value, parsed by hand from the `Cookie` header.
pub(crate) fn session_cookie(headers: &HeaderMap) -> Option<String> {
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
pub(crate) fn session_set_cookie(value: &str, max_age_s: i64, secure: bool) -> String {
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
                .token_with_user(&hash, now)
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

/// One prominent warning per process while auth runs open (zero real
/// users): the synthetic admin principal means anyone who can reach the API
/// is admin. The bind guard already refuses non-loopback binds in this
/// state; this makes the state visible in the logs too.
fn warn_open_auth_once() {
    static WARNED: Once = Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            "authentication is disabled (zero users): anyone with network access is admin; \
             create a user (`tumultd create-admin`) to enable authentication"
        );
    });
}

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
            warn_open_auth_once();
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
pub(crate) async fn exec_auth_write(
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

#[cfg(test)]
mod tests {
    use super::*;

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
