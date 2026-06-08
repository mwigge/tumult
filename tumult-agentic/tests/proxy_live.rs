//! End-to-end proof that the fault-injecting proxy mutates *live* HTTP traffic.
//!
//! A mock upstream stands in for a model provider (no API keys, no network),
//! the proxy sits in front of it with a chosen scenario pack, and a plain HTTP
//! client (standing in for Claude Code / Codex / OpenCode / Copilot) drives
//! requests through the proxy. We assert the fault actually changed what the
//! client received.

use std::net::SocketAddr;

use axum::{routing::post, Router};
use tumult_agentic::proxy::{router, ProxyConfig};

/// Start a mock upstream that always returns a known-good JSON body.
async fn start_upstream() -> SocketAddr {
    let app = Router::new().route(
        "/v1/messages",
        post(|| async { r#"{"ok":true,"answer":"the sky is blue"}"# }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("upstream addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

/// Start the proxy for `scenario_pack` in front of `upstream` and return its
/// address.
async fn start_proxy(upstream: SocketAddr, scenario_pack: &str) -> SocketAddr {
    let config = ProxyConfig {
        upstream: format!("http://{upstream}"),
        scenario_pack: scenario_pack.to_string(),
        journal_path: None,
        seed: 1,
        client: tumult_otel::agentic::TumultClient::Unknown,
    };
    let app = router(config).expect("build proxy router");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind proxy");
    let addr = listener.local_addr().expect("proxy addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

async fn post_through(proxy: SocketAddr) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{proxy}/v1/messages"))
        .header("content-type", "application/json")
        .body(r#"{"model":"demo","messages":[]}"#)
        .send()
        .await
        .expect("request through proxy")
}

#[tokio::test]
async fn malformed_pack_corrupts_the_live_response_body() {
    let upstream = start_upstream().await;
    let proxy = start_proxy(upstream, "malformed-json-recovery").await;

    let response = post_through(proxy).await;
    assert!(response.status().is_success());
    let body = response.text().await.expect("body");

    // The upstream returned valid JSON; the malformed_output fault corrupted it.
    assert_eq!(body, "{malformed-json");
    assert!(serde_json::from_str::<serde_json::Value>(&body).is_err());
}

#[tokio::test]
async fn concurrency_storm_rate_limits_without_calling_upstream() {
    let upstream = start_upstream().await;
    let proxy = start_proxy(upstream, "concurrency-storm").await;

    let response = post_through(proxy).await;
    // RateLimit short-circuits to a synthetic 429 with a retry-after header.
    assert_eq!(response.status().as_u16(), 429);
    assert!(response.headers().get("retry-after").is_some());
    let body = response.text().await.expect("body");
    assert!(body.contains("rate_limit_error"));
}

#[tokio::test]
async fn hallucination_pack_returns_synthetic_timeout() {
    let upstream = start_upstream().await;
    let proxy = start_proxy(upstream, "hallucination-under-timeout").await;

    let response = post_through(proxy).await;
    // ModelTimeout short-circuits to a 504 before reaching the upstream.
    assert_eq!(response.status().as_u16(), 504);
}

#[tokio::test]
async fn retrieval_poisoning_contaminates_the_live_body() {
    let upstream = start_upstream().await;
    let proxy = start_proxy(upstream, "retrieval-poisoning").await;

    let response = post_through(proxy).await;
    assert!(response.status().is_success());
    let body = response.text().await.expect("body");
    assert!(
        body.contains("poisoned-document-0"),
        "poison must reach the client, got: {body}"
    );
}

/// Mock upstream that echoes back the `traceparent` it received, so the test
/// can assert the proxy continued the inbound trace.
async fn start_echo_upstream() -> SocketAddr {
    let app = Router::new().route(
        "/v1/messages",
        post(|headers: axum::http::HeaderMap| async move {
            let tp = headers
                .get("traceparent")
                .and_then(|value| value.to_str().ok())
                .unwrap_or("none")
                .to_string();
            format!(r#"{{"traceparent":"{tp}"}}"#)
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind echo upstream");
    let addr = listener.local_addr().expect("echo addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

#[tokio::test]
async fn proxy_continues_inbound_trace_context_upstream() {
    // A real tracer provider so the proxy's span has a valid context to inject.
    let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
    opentelemetry::global::set_tracer_provider(provider);

    let upstream = start_echo_upstream().await;
    // cost-explosion-detector forwards the body unchanged (no body mutation, no
    // short-circuit), so the echoed traceparent reaches the client intact.
    let proxy = start_proxy(upstream, "cost-explosion-detector").await;

    let trace_id = "0af7651916cd43dd8448eb211c80319c";
    let inbound = format!("00-{trace_id}-b7ad6b7169203331-01");
    let body = reqwest::Client::new()
        .post(format!("http://{proxy}/v1/messages"))
        .header("traceparent", &inbound)
        .header("content-type", "application/json")
        .body(r#"{"model":"demo"}"#)
        .send()
        .await
        .expect("request through proxy")
        .text()
        .await
        .expect("body");

    // The upstream received a traceparent, and it continues the inbound trace
    // (same trace id, new span id) — i.e. the proxy span nested under the client.
    assert!(
        body.contains(trace_id),
        "upstream should receive a traceparent continuing the inbound trace: {body}"
    );
    assert!(
        !body.contains(&inbound),
        "proxy should inject its own child span context, not echo the inbound one verbatim: {body}"
    );
}

#[tokio::test]
async fn cost_pack_forwards_unchanged_at_the_http_layer() {
    // Token/retry faults are agent-internal: the proxy records them but cannot
    // inject them into raw HTTP, so the upstream body passes through intact.
    let upstream = start_upstream().await;
    let proxy = start_proxy(upstream, "cost-explosion-detector").await;

    let response = post_through(proxy).await;
    assert!(response.status().is_success());
    let body = response.text().await.expect("body");
    assert_eq!(body, r#"{"ok":true,"answer":"the sky is blue"}"#);
}
