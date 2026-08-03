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

/// Mint the W3C `traceparent` value for `context`'s active span.
///
/// Returns `None` when the context has no valid span (e.g. no tracer provider
/// is installed). Use this to pass `TRACEPARENT` to a child process such as
/// `claude -p`, so the agent's spans nest under a tumult experiment span.
#[must_use]
pub fn current_traceparent(context: &Context) -> Option<String> {
    let mut headers = HashMap::new();
    inject_traceparent(context, &mut headers);
    headers.remove("traceparent")
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

    #[test]
    fn current_traceparent_mints_value_for_remote_context() {
        let cx = remote_context();
        let tp = current_traceparent(&cx).expect("traceparent");
        assert!(tp.starts_with("00-0af7651916cd43dd8448eb211c80319c-"));
        assert!(current_traceparent(&Context::new()).is_none());
    }

    #[test]
    fn header_extractor_reads_values_and_lists_keys() {
        let mut headers = HashMap::new();
        headers.insert("traceparent".to_string(), "00-x".to_string());
        headers.insert("x-other".to_string(), "1".to_string());

        let extractor = HeaderExtractor(&headers);
        assert_eq!(extractor.get("traceparent"), Some("00-x"));
        assert_eq!(extractor.get("absent"), None);

        let mut keys = extractor.keys();
        keys.sort_unstable();
        assert_eq!(keys, ["traceparent", "x-other"]);
    }

    #[test]
    fn inject_then_parse_round_trips_tracestate() {
        let span = SpanContext::new(
            TraceId::from_hex("0af7651916cd43dd8448eb211c80319c").unwrap(),
            SpanId::from_hex("b7ad6b7169203331").unwrap(),
            TraceFlags::SAMPLED,
            true,
            TraceState::from_key_value([("congo", "t61rcWkgMzE")]).unwrap(),
        );
        let cx = Context::new().with_remote_span_context(span);

        let mut headers = HashMap::new();
        inject_traceparent(&cx, &mut headers);
        assert_eq!(
            headers.get("tracestate").map(String::as_str),
            Some("congo=t61rcWkgMzE")
        );

        let extracted = parse_traceparent(&headers);
        let span = extracted.span().span_context().clone();
        assert!(span.is_remote());
        assert_eq!(span.trace_state().get("congo"), Some("t61rcWkgMzE"));
    }
}
