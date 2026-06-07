//! W3C trace-context (`traceparent`/`tracestate`) propagation helpers.
//!
//! These are the single home for reading and writing W3C trace context across
//! HTTP boundaries. The agentic proxy (model-API surface) and the MCP server
//! (tool surface) both use these so inbound context becomes a span parent and
//! outbound requests continue the same distributed trace.
#![allow(clippy::doc_markdown)] // module doc names standards (W3C, OpenTelemetry)

use std::collections::HashMap;
use std::hash::BuildHasher;

use opentelemetry::propagation::{Extractor, Injector, TextMapPropagator};
use opentelemetry::Context;
use opentelemetry_sdk::propagation::TraceContextPropagator;

/// Read-only carrier over a lowercase header map.
struct HeaderExtractor<'a, S>(&'a HashMap<String, String, S>);

impl<S: BuildHasher> Extractor for HeaderExtractor<'_, S> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(String::as_str).collect()
    }
}

/// Mutable carrier over a header map.
struct HeaderInjector<'a, S>(&'a mut HashMap<String, String, S>);

impl<S: BuildHasher> Injector for HeaderInjector<'_, S> {
    fn set(&mut self, key: &str, value: String) {
        self.0.insert(key.to_string(), value);
    }
}

/// Extract a W3C trace context from request headers into an OpenTelemetry
/// [`Context`].
///
/// `headers` keys MUST be lowercase (`traceparent`, `tracestate`). When no
/// valid `traceparent` is present the returned context has no remote span, so
/// callers can treat it as "start a standalone span."
#[must_use]
pub fn parse_traceparent<S: BuildHasher>(headers: &HashMap<String, String, S>) -> Context {
    TraceContextPropagator::new().extract(&HeaderExtractor(headers))
}

/// Inject the active span of `context` into `headers` as a W3C `traceparent`
/// (and `tracestate` when present), so a downstream/upstream service continues
/// the same trace.
pub fn inject_traceparent<S: BuildHasher>(
    context: &Context,
    headers: &mut HashMap<String, String, S>,
) {
    TraceContextPropagator::new().inject_context(context, &mut HeaderInjector(headers));
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::{
        SpanContext, SpanId, TraceContextExt, TraceFlags, TraceId, TraceState,
    };

    fn remote_context() -> Context {
        let span = SpanContext::new(
            TraceId::from_hex("0af7651916cd43dd8448eb211c80319c").unwrap(),
            SpanId::from_hex("b7ad6b7169203331").unwrap(),
            TraceFlags::SAMPLED,
            true,
            TraceState::default(),
        );
        Context::new().with_remote_span_context(span)
    }

    #[test]
    fn inject_then_parse_round_trips_trace_id() {
        let cx = remote_context();
        let mut headers = HashMap::new();
        inject_traceparent(&cx, &mut headers);

        assert!(headers.contains_key("traceparent"));

        let extracted = parse_traceparent(&headers);
        let span = extracted.span().span_context().clone();
        assert!(span.is_remote());
        assert_eq!(
            span.trace_id(),
            TraceId::from_hex("0af7651916cd43dd8448eb211c80319c").unwrap()
        );
        assert_eq!(
            span.span_id(),
            SpanId::from_hex("b7ad6b7169203331").unwrap()
        );
    }

    #[test]
    fn parse_without_traceparent_yields_invalid_span() {
        let extracted = parse_traceparent(&HashMap::new());
        assert!(!extracted.span().span_context().is_valid());
    }
}
