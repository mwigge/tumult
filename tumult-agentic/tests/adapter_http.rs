//! Behavior tests for [`HttpAgentAdapter`] against a live local HTTP server:
//! success mapping, error paths (allowlist, status, decode, timeout,
//! unreachable), and trace-context propagation.

use std::time::Duration;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::{Json, Router};
use tumult_agentic::adapters::{HttpAgentAdapter, TraceContext};
use tumult_agentic::model::{AgenticError, AgenticScenario};

fn scenario(name: &str) -> AgenticScenario {
    AgenticScenario {
        name: name.to_string(),
        input: "summarise the incident".to_string(),
        expected_behavior: Some("returns a grounded summary".to_string()),
    }
}

async fn serve(app: Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test server");
    let addr = listener.local_addr().expect("server addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

#[tokio::test]
async fn invoke_maps_the_wire_response_and_floors_latency_at_elapsed() {
    let app = Router::new().route(
        "/agent",
        post(|| async {
            Json(serde_json::json!({
                "body": "grounded summary",
                "latency_ms": 0,
                "tool_calls": 2,
                "retry_count": 1,
                "input_tokens": 11,
                "output_tokens": 7,
                "fallback_used": true
            }))
        }),
    );
    let addr = serve(app).await;
    let adapter = HttpAgentAdapter::new(format!("http://{addr}/agent"));

    let response = adapter
        .invoke(&scenario("http-success"))
        .await
        .expect("invoke succeeds");

    assert_eq!(response.body, "grounded summary");
    assert_eq!(response.tool_calls, 2);
    assert_eq!(response.retry_count, 1);
    assert_eq!(response.input_tokens, 11);
    assert_eq!(response.output_tokens, 7);
    assert!(response.fallback_used);
    // The server claimed 0ms; the adapter floors latency at real elapsed time,
    // so a lying server cannot report impossibly fast calls.
    assert!(response.latency_ms <= 2_000);
}

#[tokio::test]
async fn invoke_defaults_missing_wire_fields() {
    let app = Router::new().route(
        "/agent",
        post(|| async { Json(serde_json::json!({"body": "ok", "latency_ms": 123})) }),
    );
    let addr = serve(app).await;
    let adapter = HttpAgentAdapter::new(format!("http://{addr}/agent"));

    let response = adapter
        .invoke(&scenario("http-minimal"))
        .await
        .expect("invoke succeeds");

    assert_eq!(response.body, "ok");
    assert!(response.latency_ms >= 123);
    assert_eq!(response.tool_calls, 0);
    assert!(!response.fallback_used);
}

#[tokio::test]
async fn invoke_sends_the_traceparent_header_when_configured() {
    async fn handler(headers: HeaderMap) -> Json<serde_json::Value> {
        let traceparent = headers
            .get("traceparent")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        Json(serde_json::json!({"body": traceparent, "latency_ms": 1}))
    }
    let app = Router::new().route("/agent", post(handler));
    let addr = serve(app).await;
    let adapter =
        HttpAgentAdapter::new(format!("http://{addr}/agent")).with_trace_context(TraceContext {
            traceparent: "00-11111111111111111111111111111111-2222222222222222-01".to_string(),
        });

    let response = adapter
        .invoke(&scenario("http-trace"))
        .await
        .expect("invoke succeeds");

    assert_eq!(
        response.body,
        "00-11111111111111111111111111111111-2222222222222222-01"
    );
}

#[tokio::test]
async fn invoke_posts_the_scenario_metadata_as_json() {
    #[derive(Clone)]
    struct Seen(std::sync::Arc<tokio::sync::Mutex<Option<serde_json::Value>>>);

    async fn handler(
        State(seen): State<Seen>,
        Json(payload): Json<serde_json::Value>,
    ) -> Json<serde_json::Value> {
        *seen.0.lock().await = Some(payload);
        Json(serde_json::json!({"body": "ok", "latency_ms": 1}))
    }
    let seen = Seen(std::sync::Arc::new(tokio::sync::Mutex::new(None)));
    let app = Router::new()
        .route("/agent", post(handler))
        .with_state(seen.clone());
    let addr = serve(app).await;
    let adapter = HttpAgentAdapter::new(format!("http://{addr}/agent"));

    adapter
        .invoke(&scenario("http-payload"))
        .await
        .expect("invoke succeeds");

    let payload = seen.0.lock().await.clone().expect("request reached server");
    assert_eq!(payload["scenario"], "http-payload");
    assert_eq!(
        payload["input_length"],
        serde_json::json!("summarise the incident".len())
    );
    assert_eq!(payload["expected_behavior"], "returns a grounded summary");
    // The raw prompt input is never sent — only its length.
    assert!(payload.get("input").is_none());
}

#[tokio::test]
async fn invoke_rejects_an_endpoint_outside_the_allowlist() {
    let adapter = HttpAgentAdapter::new("http://192.0.2.1/agent")
        .with_allowlist(vec!["http://allowed.example".to_string()]);

    let error = adapter
        .invoke(&scenario("http-not-allowed"))
        .await
        .expect_err("non-allowlisted endpoint must fail before any network I/O");

    assert_eq!(
        error,
        AgenticError::TargetNotAllowed("http://192.0.2.1/agent".to_string())
    );
}

#[tokio::test]
async fn invoke_accepts_an_allowlisted_endpoint_prefix() {
    let app = Router::new().route(
        "/agent",
        post(|| async { Json(serde_json::json!({"body": "ok", "latency_ms": 1})) }),
    );
    let addr = serve(app).await;
    let endpoint = format!("http://{addr}/agent");
    let adapter = HttpAgentAdapter::new(endpoint.clone())
        .with_allowlist(vec!["https://other.example".to_string(), endpoint.clone()]);

    adapter
        .invoke(&scenario("http-allowlisted"))
        .await
        .expect("allowlisted endpoint succeeds");
}

#[tokio::test]
async fn invoke_errors_on_a_non_success_status() {
    let app = Router::new().route(
        "/agent",
        post(|| async { (axum::http::StatusCode::SERVICE_UNAVAILABLE, "down") }),
    );
    let addr = serve(app).await;
    let adapter = HttpAgentAdapter::new(format!("http://{addr}/agent"));

    let error = adapter
        .invoke(&scenario("http-status"))
        .await
        .expect_err("503 must fail");

    assert_eq!(
        error,
        AgenticError::Adapter(
            "adapter=http scenario=http-status error=status status=503 Service Unavailable"
                .to_string()
        )
    );
}

#[tokio::test]
async fn invoke_errors_on_an_undecodable_response_body() {
    let app = Router::new().route("/agent", post(|| async { "this is not json" }));
    let addr = serve(app).await;
    let adapter = HttpAgentAdapter::new(format!("http://{addr}/agent"));

    let error = adapter
        .invoke(&scenario("http-decode"))
        .await
        .expect_err("invalid JSON must fail");

    match error {
        AgenticError::Adapter(message) => {
            assert!(
                message.contains("scenario=http-decode") && message.contains("error=decode"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected adapter error, got {other:?}"),
    }
}

#[tokio::test]
async fn invoke_errors_when_the_endpoint_is_unreachable() {
    // Bind and immediately drop a listener to get an address nothing serves.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let adapter = HttpAgentAdapter::new(format!("http://{addr}/agent"))
        .with_timeout(Duration::from_millis(500));

    let error = adapter
        .invoke(&scenario("http-unreachable"))
        .await
        .expect_err("connection refused must fail");

    match error {
        AgenticError::Adapter(message) => {
            assert!(
                message.contains("scenario=http-unreachable") && message.contains("error=request"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected adapter error, got {other:?}"),
    }
}

#[tokio::test]
async fn invoke_times_out_when_the_server_stalls() {
    let app = Router::new().route(
        "/agent",
        post(|| async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Json(serde_json::json!({"body": "too late", "latency_ms": 1}))
        }),
    );
    let addr = serve(app).await;
    let adapter = HttpAgentAdapter::new(format!("http://{addr}/agent"))
        .with_timeout(Duration::from_millis(100));

    let started = std::time::Instant::now();
    let error = adapter
        .invoke(&scenario("http-timeout"))
        .await
        .expect_err("a stalled server must time out");

    assert_eq!(
        error,
        AgenticError::Adapter(
            "adapter=http scenario=http-timeout error=timeout timeout_ms=100".to_string()
        )
    );
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "the timeout must bound the call; took {:?}",
        started.elapsed()
    );
}
