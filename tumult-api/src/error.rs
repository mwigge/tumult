//! JSON error-response helpers shared by the HTTP handlers: one idiom
//! crate-wide — `(StatusCode, Json({"error": msg}))` as a `Response`.
//! (`sql_util::internal` covers the 500 case, where the client gets a
//! fixed generic body and the detail is logged server-side.)

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

fn error(status: StatusCode, msg: impl Into<String>) -> Response {
    let msg: String = msg.into();
    (status, Json(json!({"error": msg}))).into_response()
}

/// 400 JSON error response. Client errors are always safe to detail: the
/// message describes the request, never store internals.
pub(crate) fn bad_request(msg: impl Into<String>) -> Response {
    error(StatusCode::BAD_REQUEST, msg)
}

/// 403 JSON error response (the requested environment/role is outside the
/// principal's scopes).
pub(crate) fn forbidden(msg: impl Into<String>) -> Response {
    error(StatusCode::FORBIDDEN, msg)
}

/// 404 JSON error response.
pub(crate) fn not_found(msg: impl Into<String>) -> Response {
    error(StatusCode::NOT_FOUND, msg)
}

/// 409 JSON error response (the request conflicts with current state).
pub(crate) fn conflict(msg: impl Into<String>) -> Response {
    error(StatusCode::CONFLICT, msg)
}

/// 503 JSON error response (the endpoint is not wired to the daemon's
/// writer/queue).
pub(crate) fn unavailable(msg: impl Into<String>) -> Response {
    error(StatusCode::SERVICE_UNAVAILABLE, msg)
}
