//! JSON error-response helpers shared by the HTTP handlers: one idiom
//! crate-wide — `(StatusCode, Json({"error": msg}))` wrapped in a small [`ApiError`].
//! (`sql_util::internal` covers the 500 case, where the client gets a
//! fixed generic body and the detail is logged server-side.)

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// A small error value that preserves the original HTTP response verbatim.
#[derive(Debug)]
pub struct ApiError(Box<Response>);

impl From<Response> for ApiError {
    fn from(response: Response) -> Self {
        Self(Box::new(response))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        *self.0
    }
}

fn error(status: StatusCode, msg: impl Into<String>) -> ApiError {
    let msg: String = msg.into();
    (status, Json(json!({"error": msg}))).into_response().into()
}

/// 400 JSON error response. Client errors are always safe to detail: the
/// message describes the request, never store internals.
pub(crate) fn bad_request(msg: impl Into<String>) -> ApiError {
    error(StatusCode::BAD_REQUEST, msg)
}

/// 403 JSON error response (the requested environment/role is outside the
/// principal's scopes).
pub(crate) fn forbidden(msg: impl Into<String>) -> ApiError {
    error(StatusCode::FORBIDDEN, msg)
}

/// 404 JSON error response.
pub(crate) fn not_found(msg: impl Into<String>) -> ApiError {
    error(StatusCode::NOT_FOUND, msg)
}

/// 409 JSON error response (the request conflicts with current state).
pub(crate) fn conflict(msg: impl Into<String>) -> ApiError {
    error(StatusCode::CONFLICT, msg)
}

/// 503 JSON error response (the endpoint is not wired to the daemon's
/// writer/queue).
pub(crate) fn unavailable(msg: impl Into<String>) -> ApiError {
    error(StatusCode::SERVICE_UNAVAILABLE, msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn api_error_is_small_and_preserves_the_complete_response() {
        assert_eq!(
            std::mem::size_of::<ApiError>(),
            std::mem::size_of::<usize>()
        );
        let mut original = (
            StatusCode::CONFLICT,
            [("x-request-id", "request-42")],
            "unchanged body",
        )
            .into_response();
        original.extensions_mut().insert(42_u64);
        let response = ApiError::from(original).into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(response.headers()["x-request-id"], "request-42");
        assert_eq!(response.extensions().get::<u64>(), Some(&42));
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), b"unchanged body");
    }
}
