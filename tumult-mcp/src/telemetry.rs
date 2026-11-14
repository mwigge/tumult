//! `OTel` instrumentation for MCP tool dispatch.

use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};
use tumult_otel::SpanGuard;

const TRACER: &str = "tumult-mcp";

/// Longest tool name written to a log event, in characters. Every real tool
/// name is far shorter; the name is client input, so anything longer is cut
/// rather than letting a caller write arbitrarily large log lines.
pub(crate) const MAX_LOGGED_TOOL_NAME_CHARS: usize = 64;

/// Logs a tool invocation under the `gen_ai.tool.name` key, with the
/// client-supplied name cut to [`MAX_LOGGED_TOOL_NAME_CHARS`].
pub(crate) fn log_tool_call(tool_name: &str) {
    tracing::info!(
        name: "mcp.tool.called",
        { gen_ai.tool.name = bounded_tool_name(tool_name) },
        "MCP tool call"
    );
}

/// The first [`MAX_LOGGED_TOOL_NAME_CHARS`] characters of `name`.
fn bounded_tool_name(name: &str) -> &str {
    match name.char_indices().nth(MAX_LOGGED_TOOL_NAME_CHARS) {
        Some((end, _)) => &name[..end],
        None => name,
    }
}

/// Start a span for MCP tool invocation.
pub(crate) fn begin_tool_call(tool_name: &str) -> SpanGuard {
    let tracer = global::tracer(TRACER);
    let span = tracer
        .span_builder("mcp.tool.call")
        .with_kind(SpanKind::Server)
        .with_attributes(vec![
            KeyValue::new("mcp.tool.name", tool_name.to_string()),
            KeyValue::new("rpc.method", "tools/call"),
            KeyValue::new("rpc.system", "mcp"),
        ])
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    SpanGuard::new(cx.attach())
}

/// Capture the currently-active OpenTelemetry context so it can be passed across
/// an async boundary (e.g. to `RunConfig::parent_context`).
///
/// Call this while a `SpanGuard` returned by [`begin_tool_call`] is still
/// in scope. The returned `Context` can be stored in `RunConfig` and used
/// as a parent for the `resilience.experiment` root span, linking the
/// experiment trace to the originating MCP tool call.
#[must_use]
pub(crate) fn current_context() -> opentelemetry::Context {
    opentelemetry::Context::current()
}

/// Record an `mcp.tool.completed` span event on the currently-active span,
/// mapping `success` to an `rpc.grpc.status_code` attribute (0 = OK, 2 = UNKNOWN).
pub(crate) fn event_tool_completed(tool_name: &str, success: bool) {
    let cx = opentelemetry::Context::current();
    // rpc.grpc.status_code: 0 = OK, 2 = UNKNOWN (used as generic failure)
    let grpc_status: i64 = if success { 0 } else { 2 };
    cx.span().add_event(
        "mcp.tool.completed",
        vec![
            KeyValue::new("mcp.tool.name", tool_name.to_string()),
            KeyValue::new("mcp.tool.success", success),
            KeyValue::new("rpc.grpc.status_code", grpc_status),
        ],
    );
}

/// Record an `mcp.tool.error` span event carrying the error message on the
/// currently-active span.
pub(crate) fn event_tool_error(tool_name: &str, error: &str) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "mcp.tool.error",
        vec![
            KeyValue::new("mcp.tool.name", tool_name.to_string()),
            KeyValue::new("mcp.tool.error", error.to_string()),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_call_span_does_not_panic() {
        let _g = begin_tool_call("tumult_run_experiment");
        event_tool_completed("tumult_run_experiment", true);
    }

    #[test]
    fn tool_error_event_does_not_panic() {
        let _g = begin_tool_call("tumult_analyze");
        event_tool_error("tumult_analyze", "query failed: syntax error");
    }

    #[test]
    fn current_context_captured_while_span_active() {
        // The context captured inside the span guard scope contains the active
        // span; after the guard drops the context still retains the trace info.
        let cx = {
            let _g = begin_tool_call("tumult_run_experiment");
            current_context()
        };
        // Context must be a valid (non-panicking) value regardless of provider.
        // With a noop provider the span is invalid but context is still present.
        let _ = cx; // non-panicking assertion
    }

    #[test]
    fn a_tool_call_is_logged_under_the_gen_ai_tool_name() {
        let logs = tumult_otel::logging::testing::capture_json(tracing::Level::INFO, || {
            log_tool_call("tumult_validate");
        });
        let called = logs.named("mcp.tool.called");
        assert_eq!(called.len(), 1, "{}", logs.raw);
        assert_eq!(called[0]["gen_ai.tool.name"], "tumult_validate");
        assert_eq!(called[0]["message"], "MCP tool call");
    }

    #[test]
    fn a_client_supplied_tool_name_is_bounded_before_it_is_logged() {
        let hostile = format!("tumult_{}\u{e9}", "x".repeat(4096));
        let logs = tumult_otel::logging::testing::capture_json(tracing::Level::INFO, || {
            log_tool_call(&hostile);
        });
        let logged = logs.events[0]["gen_ai.tool.name"].as_str().unwrap();
        assert_eq!(logged.chars().count(), MAX_LOGGED_TOOL_NAME_CHARS);
        assert!(hostile.starts_with(logged));
    }
}
