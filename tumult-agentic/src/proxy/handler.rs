//! The fallback request handler that drives fault injection per request.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{Request, State};
use axum::http::{header::CONTENT_TYPE, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::faults::{FaultEngine, FaultSpec};

use super::config::{ProxyState, MAX_BODY_BYTES};
use super::forward::forward;
use super::inject::{classify, error_body, mutate_body, Injection};
use super::journal::record;
use super::response::{elapsed_ms, synthetic, ResponseNote};

/// Project request headers into a lowercase string map for trace-context
/// extraction (`traceparent`/`tracestate` are matched lowercase).
fn header_map_lower(headers: &axum::http::HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect()
}

pub(crate) async fn handle(State(state): State<Arc<ProxyState>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let request_body = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(err) => {
            return synthetic(
                StatusCode::PAYLOAD_TOO_LARGE,
                error_body("request_too_large"),
                None,
            )
            .into_response_with_note(&format!("read body failed: {err}"));
        }
    };

    let seq = state.counter.fetch_add(1, Ordering::Relaxed);
    let mut engine = FaultEngine::new(state.seed.wrapping_add(seq));

    let mut delay = Duration::ZERO;
    let mut short_circuit: Option<(u16, String, Option<u64>)> = None;
    let mut body_faults: Vec<FaultSpec> = Vec::new();
    let mut applied: Vec<String> = Vec::new();

    for fault in &state.faults {
        if !engine.should_apply(fault) {
            continue;
        }
        applied.push(fault.fault_type().to_string());
        match classify(fault) {
            Injection::Delay(duration) => delay = delay.saturating_add(duration),
            Injection::ShortCircuit {
                status,
                body,
                retry_after_ms,
            } => {
                if short_circuit.is_none() {
                    short_circuit = Some((status, body, retry_after_ms));
                }
            }
            Injection::MutateBody(spec) => body_faults.push(spec),
            Injection::Internal => {}
        }
    }

    // Start a fault span parented under the client's inbound trace context (or
    // standalone, tagged by tumult.client, when the client did not propagate).
    let parent = tumult_otel::propagation::parse_traceparent(&header_map_lower(&parts.headers));
    let proxy_span = tumult_otel::agentic_span::start_proxy_span(
        &parent,
        state.tumult_client.as_str(),
        &state.scenario,
        parts.method.as_str(),
        parts.uri.path(),
    );

    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }

    let started = Instant::now();

    // Short-circuit faults never touch the upstream.
    if let Some((status, body, retry_after_ms)) = short_circuit {
        let elapsed = elapsed_ms(started, delay);
        record(&state, &parts, &applied, status, &body, elapsed);
        proxy_span.set_outcome(status, elapsed, &applied);
        proxy_span.end();
        return synthetic(
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY),
            body,
            retry_after_ms,
        );
    }

    // Forward to the upstream, then mutate the response body.
    let path_and_query = parts
        .uri
        .path_and_query()
        .map_or_else(|| parts.uri.path().to_string(), ToString::to_string);
    let url = format!("{}{path_and_query}", state.upstream);

    let upstream = match forward(
        &state.client,
        &parts,
        &url,
        request_body,
        proxy_span.context(),
    )
    .await
    {
        Ok(response) => response,
        Err(err) => {
            let body = error_body("upstream_unreachable");
            let elapsed = elapsed_ms(started, delay);
            record(&state, &parts, &applied, 502, &body, elapsed);
            proxy_span.set_outcome(502, elapsed, &applied);
            proxy_span.end();
            return synthetic(StatusCode::BAD_GATEWAY, body, None)
                .into_response_with_note(&err.to_string());
        }
    };

    let mut response_body = upstream.body;
    for fault in &body_faults {
        response_body = mutate_body(fault, response_body);
    }

    let elapsed = elapsed_ms(started, delay);
    record(
        &state,
        &parts,
        &applied,
        upstream.status,
        &response_body,
        elapsed,
    );
    proxy_span.set_outcome(upstream.status, elapsed, &applied);
    proxy_span.end();

    let status = StatusCode::from_u16(upstream.status).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut response = (status, response_body).into_response();
    if let Some(content_type) = upstream.content_type {
        if let Ok(value) = HeaderValue::from_str(&content_type) {
            response.headers_mut().insert(CONTENT_TYPE, value);
        }
    }
    response
}
