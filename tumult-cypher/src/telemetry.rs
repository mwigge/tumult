//! `OTel` instrumentation for the cypher query engine.
//!
//! One span per query: `cypher.query` covers the whole disposable-engine
//! lifecycle (snapshot → graph rebuild → execution → row mapping). The query
//! text is never recorded — it comes from agents and may embed user data —
//! so the span carries only sizes and the outcome.

use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};

const TRACER_NAME: &str = "tumult-cypher";

/// Start a span for one openCypher query execution. Returns a context
/// guard — the span ends on drop.
#[must_use]
pub(crate) fn begin_query(node_count: usize, edge_count: usize, row_cap: usize) -> QueryGuard {
    let tracer = global::tracer(TRACER_NAME);
    let span = tracer
        .span_builder("cypher.query")
        .with_kind(SpanKind::Internal)
        .with_attributes(vec![
            KeyValue::new(
                "cypher.snapshot.nodes",
                i64::try_from(node_count).unwrap_or(i64::MAX),
            ),
            KeyValue::new(
                "cypher.snapshot.edges",
                i64::try_from(edge_count).unwrap_or(i64::MAX),
            ),
            KeyValue::new("cypher.row_cap", i64::try_from(row_cap).unwrap_or(i64::MAX)),
        ])
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    QueryGuard {
        _guard: cx.attach(),
    }
}

pub(crate) struct QueryGuard {
    _guard: opentelemetry::ContextGuard,
}

/// Record the outcome of a successful query on the active `cypher.query`
/// span: returned row count (post-cap), truncation flag, and wall-clock
/// duration of the whole rebuild-plus-execute cycle.
pub(crate) fn record_query_result(rows: usize, truncated: bool, duration: std::time::Duration) {
    let cx = opentelemetry::Context::current();
    let span = cx.span();
    span.set_attribute(KeyValue::new(
        "cypher.result.rows",
        i64::try_from(rows).unwrap_or(i64::MAX),
    ));
    span.set_attribute(KeyValue::new("cypher.result.truncated", truncated));
    span.set_attribute(KeyValue::new(
        "cypher.duration_ms",
        duration.as_secs_f64() * 1_000.0,
    ));
}

/// Record a failed query on the active span as an error status.
pub(crate) fn record_query_error(error: &str) {
    let cx = opentelemetry::Context::current();
    cx.span()
        .set_status(opentelemetry::trace::Status::error(error.to_string()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_span_lifecycle_does_not_panic() {
        let _g = begin_query(10, 20, 500);
        record_query_result(7, false, std::time::Duration::from_millis(3));
        record_query_error("boom");
    }
}
