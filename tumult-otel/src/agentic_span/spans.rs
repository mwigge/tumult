//! Live span wrappers and starters: the proxy fault span, the parentable span
//! scope, and the experiment-root / tool-surface / proxy span constructors.

use opentelemetry::trace::{SpanKind, Status, TraceContextExt, Tracer};
use opentelemetry::{global, Context, KeyValue};

use super::SCOPE;
use crate::agentic;

pub const PROXY_SPAN: &str = "tumult.agentic.fault";
pub const HTTP_METHOD: &str = "http.request.method";
pub const HTTP_PATH: &str = "url.path";
pub const HTTP_STATUS: &str = "http.response.status_code";
pub const DURATION_MS: &str = "resilience.duration_ms";
pub const FAULTS_INJECTED: &str = "resilience.agent.faults_injected";

/// A live span wrapping one proxied request, parented under the client's inbound
/// trace context. The proxy injects this span's `traceparent` upstream, then
/// records the outcome and ends it. Span lifecycle stays in tumult-otel.
pub struct ProxySpan {
    context: opentelemetry::Context,
}

impl ProxySpan {
    /// The context whose active span is this proxy span — pass to
    /// [`crate::propagation::inject_traceparent`] to propagate it upstream.
    #[must_use]
    pub fn context(&self) -> &opentelemetry::Context {
        &self.context
    }

    /// Record the request outcome on the span.
    pub fn set_outcome(&self, status_code: u16, latency_ms: u64, faults: &[String]) {
        let span = self.context.span();
        span.set_attribute(KeyValue::new(HTTP_STATUS, i64::from(status_code)));
        span.set_attribute(KeyValue::new(
            DURATION_MS,
            i64::try_from(latency_ms).unwrap_or(i64::MAX),
        ));
        span.set_attribute(KeyValue::new(FAULTS_INJECTED, faults.join(",")));
        span.set_status(if status_code >= 500 {
            Status::error("upstream/injected error")
        } else {
            Status::Ok
        });
    }

    /// End the span. Consumes the wrapper so it cannot be reused.
    pub fn end(self) {
        self.context.span().end();
    }
}

/// A live span held in a parentable context, returned by the tool-surface and
/// experiment-root helpers. Attach a clone of [`context`](Self::context) as the
/// current context so descendant spans nest under it, then [`end`](Self::end).
pub struct SpanScope {
    context: Context,
}

impl SpanScope {
    /// The context whose active span is this scope's span.
    #[must_use]
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// End the span. Consumes the wrapper so it cannot be reused.
    pub fn end(self) {
        self.context.span().end();
    }
}

pub const EXPERIMENT_ROOT_SPAN: &str = "tumult.experiment";

/// Start a `tumult.experiment` root span for an orchestrated run, tagged by
/// `client`. tumult mints a `traceparent` from it (via
/// [`crate::propagation::current_traceparent`]) and passes it to the agent
/// subprocess, so the agent's spans nest under tumult's experiment.
#[must_use]
pub fn start_experiment_root(scenario: &str, client: &str) -> SpanScope {
    let tracer = global::tracer(SCOPE);
    let span = tracer
        .span_builder(EXPERIMENT_ROOT_SPAN)
        .with_kind(SpanKind::Internal)
        .with_attributes([
            KeyValue::new(agentic::RESILIENCE_AGENT_SCENARIO, scenario.to_string()),
            KeyValue::new(agentic::TUMULT_CLIENT, client.to_string()),
            KeyValue::new(agentic::RESILIENCE_AGENT_CAPTURE_POLICY, "metadata_only"),
        ])
        .start(&tracer);
    SpanScope {
        context: Context::current().with_span(span),
    }
}

pub const TOOL_SPAN: &str = "tumult.agentic.tool";

/// Start a tool-surface span for an MCP agentic tool call, tagged by `client`.
///
/// The MCP transport does not expose the inbound `traceparent` to tool handlers,
/// so this span parents under the current context (a standalone root when none
/// is active) rather than the calling agent's trace — the "correlate" tier.
/// While its context is attached as current, the experiment span emitted by
/// [`record_agentic_run`](super::record_agentic_run) nests under it (tool → experiment).
#[must_use]
pub fn start_tool_span(client: &str, tool_name: &str) -> SpanScope {
    let tracer = global::tracer(SCOPE);
    let span = tracer
        .span_builder(TOOL_SPAN)
        .with_kind(SpanKind::Server)
        .with_attributes([
            KeyValue::new(agentic::TUMULT_CLIENT, client.to_string()),
            KeyValue::new(agentic::GEN_AI_TOOL_NAME, tool_name.to_string()),
            KeyValue::new(agentic::RESILIENCE_AGENT_CAPTURE_POLICY, "metadata_only"),
        ])
        .start(&tracer);
    SpanScope {
        context: Context::current().with_span(span),
    }
}

/// Start a proxy fault span parented under `parent` (the client's inbound trace
/// context, or an empty context for a standalone span tagged by `client`).
#[must_use]
pub fn start_proxy_span(
    parent: &Context,
    client: &str,
    scenario: &str,
    method: &str,
    path: &str,
) -> ProxySpan {
    let tracer = global::tracer(SCOPE);
    let span = tracer
        .span_builder(PROXY_SPAN)
        .with_kind(SpanKind::Client)
        .with_attributes([
            KeyValue::new(agentic::TUMULT_CLIENT, client.to_string()),
            KeyValue::new(agentic::RESILIENCE_AGENT_SCENARIO, scenario.to_string()),
            KeyValue::new(agentic::RESILIENCE_AGENT_CAPTURE_POLICY, "metadata_only"),
            KeyValue::new(HTTP_METHOD, method.to_string()),
            KeyValue::new(HTTP_PATH, path.to_string()),
        ])
        .start_with_context(&tracer, parent);
    ProxySpan {
        context: parent.with_span(span),
    }
}
