//! Live span wrappers and starters: the proxy fault span, the parentable span
//! scope, and the experiment-root / tool-surface / proxy span constructors.

use opentelemetry::trace::{SpanKind, Status, TraceContextExt, Tracer};
use opentelemetry::{global, Context, KeyValue};

use super::SCOPE;
use crate::agentic;

/// Name of the span wrapping one proxied, potentially fault-injected request.
pub const PROXY_SPAN: &str = "tumult.agentic.fault";
/// Span attribute key for the request's HTTP method.
pub const HTTP_METHOD: &str = "http.request.method";
/// Span attribute key for the request URL path.
pub const HTTP_PATH: &str = "url.path";
/// Span attribute key for the response HTTP status code.
pub const HTTP_STATUS: &str = "http.response.status_code";
/// Span attribute key for the request duration, in milliseconds.
pub const DURATION_MS: &str = "resilience.duration_ms";
/// Span attribute key listing the faults injected into the request (comma-separated).
pub const FAULTS_INJECTED: &str = "resilience.agent.faults_injected";

/// A live span wrapping one proxied request, parented under the client's inbound
/// trace context. The proxy injects this span's `traceparent` upstream, then
/// records the outcome and ends it. Span lifecycle stays in tumult-otel.
///
/// Dropping the wrapper without calling [`end`](Self::end) ends the span as a
/// fallback, so an early return on a future code path cannot silently lose it.
pub struct ProxySpan {
    context: opentelemetry::Context,
    ended: bool,
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
    pub fn end(mut self) {
        self.end_once();
    }

    /// End the underlying span at most once. Shared by [`Self::end`] and the
    /// `Drop` fallback; the flag guards against a double-end when `end` was
    /// already called.
    fn end_once(&mut self) {
        if !self.ended {
            self.ended = true;
            self.context.span().end();
        }
    }
}

impl Drop for ProxySpan {
    fn drop(&mut self) {
        self.end_once();
    }
}

/// A live span held in a parentable context, returned by the tool-surface and
/// experiment-root helpers. Attach a clone of [`context`](Self::context) as the
/// current context so descendant spans nest under it, then [`end`](Self::end).
///
/// Dropping the scope without calling [`end`](Self::end) ends the span as a
/// fallback, so an early return on a future code path cannot silently lose it.
pub struct SpanScope {
    context: Context,
    ended: bool,
}

impl SpanScope {
    /// The context whose active span is this scope's span.
    #[must_use]
    pub fn context(&self) -> &Context {
        &self.context
    }

    /// End the span. Consumes the wrapper so it cannot be reused.
    pub fn end(mut self) {
        self.end_once();
    }

    /// End the underlying span at most once. Shared by [`Self::end`] and the
    /// `Drop` fallback; the flag guards against a double-end when `end` was
    /// already called.
    fn end_once(&mut self) {
        if !self.ended {
            self.ended = true;
            self.context.span().end();
        }
    }
}

impl Drop for SpanScope {
    fn drop(&mut self) {
        self.end_once();
    }
}

/// Name of the root span for an orchestrated experiment run.
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
        ended: false,
    }
}

/// Name of the span emitted for one MCP agentic tool call.
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
        ended: false,
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
        ended: false,
    }
}

#[cfg(test)]
mod tests {
    //! Drop-fallback tests build spans against a private in-memory provider
    //! instead of the global one, so parallel tests elsewhere in the crate
    //! cannot swap the global provider mid-assertion.
    use super::*;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

    fn harness() -> (SdkTracerProvider, InMemorySpanExporter) {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        (provider, exporter)
    }

    fn finished_count(provider: &SdkTracerProvider, exporter: &InMemorySpanExporter) -> usize {
        provider.force_flush().ok();
        exporter
            .get_finished_spans()
            .expect("spans")
            .iter()
            .filter(|span| span.name == "test.span")
            .count()
    }

    #[test]
    fn dropped_span_scope_ends_span_via_drop_fallback() {
        let (provider, exporter) = harness();
        let tracer = provider.tracer(SCOPE);
        let span = tracer.span_builder("test.span").start(&tracer);
        {
            let _scope = SpanScope {
                context: Context::current().with_span(span),
                ended: false,
            };
            // No end() call — the Drop fallback must end it exactly once.
        }
        assert_eq!(finished_count(&provider, &exporter), 1);
    }

    #[test]
    fn explicit_end_then_drop_ends_span_scope_only_once() {
        let (provider, exporter) = harness();
        let tracer = provider.tracer(SCOPE);
        let span = tracer.span_builder("test.span").start(&tracer);
        let scope = SpanScope {
            context: Context::current().with_span(span),
            ended: false,
        };
        scope.end();
        assert_eq!(finished_count(&provider, &exporter), 1);
    }

    #[test]
    fn dropped_proxy_span_ends_span_via_drop_fallback() {
        let (provider, exporter) = harness();
        let tracer = provider.tracer(SCOPE);
        let parent = Context::new();
        let span = tracer
            .span_builder("test.span")
            .start_with_context(&tracer, &parent);
        {
            let _proxy = ProxySpan {
                context: parent.with_span(span),
                ended: false,
            };
        }
        assert_eq!(finished_count(&provider, &exporter), 1);
    }

    #[test]
    fn explicit_end_then_drop_ends_proxy_span_only_once() {
        let (provider, exporter) = harness();
        let tracer = provider.tracer(SCOPE);
        let parent = Context::new();
        let span = tracer
            .span_builder("test.span")
            .start_with_context(&tracer, &parent);
        let proxy = ProxySpan {
            context: parent.with_span(span),
            ended: false,
        };
        proxy.end();
        assert_eq!(finished_count(&provider, &exporter), 1);
    }
}
