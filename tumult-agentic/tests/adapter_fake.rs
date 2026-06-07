use std::time::Duration;

use tumult_agentic::adapters::{
    fixture_response, trace_headers, AgentAdapter, FakeHttpAgentAdapter, FakeMcpAdapter,
    McpToolInvocation, TraceContext,
};
use tumult_agentic::model::{AgenticError, AgenticScenario};

fn scenario(name: &str) -> AgenticScenario {
    AgenticScenario {
        name: name.to_string(),
        input: "local smoke input".to_string(),
        expected_behavior: Some("deterministic fixture response".to_string()),
    }
}

#[test]
fn adapter_fake_http_returns_fixture_without_network() {
    let adapter = FakeHttpAgentAdapter::new("local-http", fixture_response(r#"{"ok":true}"#));

    let response = adapter
        .invoke(&scenario("adapter-http-success"))
        .expect("fake HTTP should respond");

    assert_eq!(response.body, r#"{"ok":true}"#);
    assert_eq!(response.latency_ms, 1);
}

#[test]
fn adapter_fake_http_timeout_error_names_adapter_and_scenario() {
    let adapter = FakeHttpAgentAdapter::new("local-http", fixture_response(r#"{"ok":true}"#))
        .with_delay(Duration::from_millis(20))
        .with_timeout(Duration::from_millis(5));

    let error = adapter
        .invoke(&scenario("adapter-http-timeout"))
        .expect_err("timeout should fail");

    assert_eq!(
        error,
        AgenticError::Adapter(
            "adapter=fake_http name=local-http scenario=adapter-http-timeout error=timeout timeout_ms=5"
                .to_string()
        )
    );
}

#[test]
fn adapter_fake_http_exposes_trace_headers() {
    let context = TraceContext {
        traceparent: "00-11111111111111111111111111111111-2222222222222222-01".to_string(),
    };
    let adapter = FakeHttpAgentAdapter::new("local-http", fixture_response(r#"{"ok":true}"#))
        .with_trace_context(context.clone());

    assert_eq!(adapter.trace_headers(), trace_headers(&context));
}

#[test]
fn adapter_fake_mcp_validates_required_fields() {
    let adapter = FakeMcpAdapter::new("local-mcp", "lookup", fixture_response(r#"{"ok":true}"#));
    let invocation = McpToolInvocation {
        input: serde_json::json!({"scenario": "adapter-mcp-missing-field"}),
        required_fields: vec!["scenario".to_string(), "query".to_string()],
        trace_context: None,
    };

    let error = adapter
        .invoke_tool(&invocation)
        .expect_err("missing query should fail");

    assert_eq!(
        error,
        AgenticError::Adapter(
            "adapter=mcp server=local-mcp tool=lookup error=missing_required_field field=query"
                .to_string()
        )
    );
}

#[test]
fn adapter_fake_mcp_simulates_tool_failure() {
    let adapter = FakeMcpAdapter::new("local-mcp", "lookup", fixture_response(r#"{"ok":true}"#))
        .with_failure("unavailable");
    let invocation = McpToolInvocation {
        input: serde_json::json!({"scenario": "adapter-mcp-tool-failure"}),
        required_fields: vec!["scenario".to_string()],
        trace_context: None,
    };

    let error = adapter
        .invoke_tool(&invocation)
        .expect_err("tool failure should fail");

    assert_eq!(
        error,
        AgenticError::Adapter(
            "adapter=mcp server=local-mcp tool=lookup error=tool_failure failure=unavailable"
                .to_string()
        )
    );
}
