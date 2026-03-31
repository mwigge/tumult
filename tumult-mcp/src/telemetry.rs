//! OTel instrumentation for MCP tool dispatch.

use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};

const TRACER: &str = "tumult-mcp";

pub struct SpanGuard {
    _guard: opentelemetry::ContextGuard,
}

/// Start a span for MCP tool invocation.
pub fn begin_tool_call(tool_name: &str) -> SpanGuard {
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
    SpanGuard {
        _guard: cx.attach(),
    }
}

pub fn event_tool_completed(tool_name: &str, success: bool) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "mcp.tool.completed",
        vec![
            KeyValue::new("mcp.tool.name", tool_name.to_string()),
            KeyValue::new("mcp.tool.success", success),
        ],
    );
}

pub fn event_tool_error(tool_name: &str, error: &str) {
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
}
