//! The compiled web UI (SvelteKit static SPA), embedded into the binary and
//! served as the fallback of the HTTP server.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// The compiled web UI (SvelteKit static SPA), embedded into the binary.
/// `web/build/` must exist at compile time — run `npm ci && npm run build`
/// in `web/` first (the Dockerfile does this in a node stage).
#[derive(rust_embed::RustEmbed)]
#[folder = "../web/build/"]
pub(super) struct UiAssets;

/// Serve the embedded SPA: real files by path, `index.html` at `/`, and the
/// `200.html` app shell for every other non-API path (client-side routing).
pub(super) async fn ui_handler(uri: axum::http::Uri) -> Response {
    use axum::http::header;
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(file) = UiAssets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        // Fingerprinted assets are safe to cache forever.
        let cache = if path.starts_with("_app/immutable/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        let mut resp = (
            [(header::CONTENT_TYPE, mime.as_ref().to_string())],
            file.data,
        )
            .into_response();
        resp.headers_mut().insert(
            header::CACHE_CONTROL,
            cache.parse().expect("static header value"),
        );
        return resp;
    }
    match UiAssets::get("200.html") {
        Some(file) => (
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            file.data,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "web UI is not embedded").into_response(),
    }
}

/// The path both the Prometheus exposition and the web UI's Metrics page use.
const METRICS_PATH: &str = "/metrics";

/// Whether the request asks for HTML first, as a browser navigation does
/// (`text/html,application/xhtml+xml,...`). Prometheus asks for
/// `application/openmetrics-text` or `text/plain` first, and curl sends
/// `*/*` or nothing: none of them want HTML.
pub(super) fn wants_html(headers: &axum::http::HeaderMap) -> bool {
    let Some(accept) = headers
        .get(axum::http::header::ACCEPT)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    let first = accept.split(',').next().unwrap_or_default();
    let media = first.split(';').next().unwrap_or_default().trim();
    media.eq_ignore_ascii_case("text/html")
}

/// `/metrics` serves Prometheus text to scrapers, and the web UI to a
/// browser that opens or reloads the Metrics page there. Layered outside
/// the auth middleware: the app shell carries no data and is served
/// without credentials like every other UI path, while every request
/// that does not ask for HTML first meets the auth rules of `/metrics`
/// unchanged.
pub(super) async fn metrics_page_for_browsers(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if req.uri().path() != METRICS_PATH || !wants_html(req.headers()) {
        return next.run(req).await;
    }
    ui_handler(req.uri().clone()).await
}
