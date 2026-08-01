// Imported from kronika. Pedantic lints are scoped to tumult-native
// crates; this file predates the pedantic gate (see crate lib.rs).
#![allow(clippy::pedantic)]

//! Unit tests for the small pure helpers of the query API (the former
//! inline `mod tests` of `lib.rs`, moved with the handlers they cover).

use axum::http::StatusCode;
use serde_json::{json, Value};
use tumult_api::handlers::metrics::{hist_quantile, valid_attr_key};
use tumult_api::sql_util::internal;

#[test]
fn hist_quantile_interpolates_within_bucket() {
    // 4 observations: 1 below 100, 2 in [100,200), 1 at/above 200.
    let counts = [1.0, 2.0, 1.0];
    let bounds = [100.0, 200.0];
    // Median falls halfway through the middle bucket.
    assert_eq!(hist_quantile(&counts, &bounds, 0.5), Some(150.0));
    // p30 lands just inside the middle bucket.
    assert_eq!(hist_quantile(&counts, &bounds, 0.3), Some(110.0));
    // p95 lands in the overflow bucket → clamps to the last bound.
    assert_eq!(hist_quantile(&counts, &bounds, 0.95), Some(200.0));
}

#[test]
fn hist_quantile_handles_empty_and_zero_buckets() {
    assert_eq!(hist_quantile(&[], &[], 0.5), None);
    assert_eq!(hist_quantile(&[0.0, 0.0], &[100.0], 0.5), None);
    // Zero-count leading buckets are skipped; the target then lands in
    // the overflow bucket and clamps to the last bound.
    assert_eq!(hist_quantile(&[0.0, 4.0], &[100.0], 0.5), Some(100.0));
}

#[test]
fn attr_key_charset_is_strict() {
    assert!(valid_attr_key("route"));
    assert!(valid_attr_key("http.route_v2"));
    assert!(!valid_attr_key(""));
    assert!(!valid_attr_key("Route"));
    assert!(!valid_attr_key("x';DROP"));
    assert!(!valid_attr_key("a b"));
}

/// 500 bodies are generic: store internals (schema, paths, DuckDB error
/// text) are logged server-side, never returned to the client.
#[tokio::test]
async fn internal_error_hides_store_details() {
    let resp = internal("duckdb: IO Error: cannot open /var/lib/tumult/k.duckdb".into());
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body, json!({"error": "internal error"}));
}
