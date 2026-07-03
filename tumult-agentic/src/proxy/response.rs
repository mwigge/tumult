//! Synthetic response construction and timing helpers.

use std::time::{Duration, Instant};

use axum::http::{header::CONTENT_TYPE, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

pub(crate) fn synthetic(status: StatusCode, body: String, retry_after_ms: Option<u64>) -> Response {
    let mut response = (status, body).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(ms) = retry_after_ms {
        let seconds = ms.div_ceil(1000).max(1);
        if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
            response.headers_mut().insert("retry-after", value);
        }
    }
    response
}

pub(crate) fn elapsed_ms(started: Instant, delay: Duration) -> u64 {
    let injected = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX);
    let measured = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    injected.saturating_add(measured)
}

/// Extension to attach a diagnostic note header to a synthetic error response.
pub(crate) trait ResponseNote {
    fn into_response_with_note(self, note: &str) -> Response;
}

impl ResponseNote for Response {
    fn into_response_with_note(mut self, note: &str) -> Response {
        if let Ok(value) = HeaderValue::from_str(note) {
            self.headers_mut().insert("x-tumult-proxy-note", value);
        }
        self
    }
}
