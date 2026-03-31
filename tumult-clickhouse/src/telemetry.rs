//! `OTel` instrumentation for `ClickHouse` backend operations.

use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};
use tumult_otel::SpanGuard;

const TRACER: &str = "tumult-clickhouse";

/// Span for `ClickHouse` connection + schema init.
pub(crate) fn begin_connect(url: &str, database: &str) -> SpanGuard {
    let tracer = global::tracer(TRACER);
    let span = tracer
        .span_builder("clickhouse.connect")
        .with_kind(SpanKind::Client)
        .with_attributes(vec![
            KeyValue::new("db.system", "clickhouse"),
            KeyValue::new("db.name", database.to_string()),
            KeyValue::new("server.address", url.to_string()),
        ])
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    SpanGuard::new(cx.attach())
}

/// Event: schema initialized.
pub(crate) fn event_schema_initialized(database: &str, version: i64) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "clickhouse.schema.initialized",
        vec![
            KeyValue::new("db.name", database.to_string()),
            KeyValue::new("db.schema.version", version),
        ],
    );
}

/// Event: `ClickHouse` DDL executed.
pub(crate) fn event_ddl_executed(statement: &str) {
    let cx = opentelemetry::Context::current();
    let preview = if statement.len() > 128 {
        format!("{}...", &statement[..128])
    } else {
        statement.to_string()
    };
    cx.span().add_event(
        "clickhouse.ddl.executed",
        vec![KeyValue::new("db.statement", preview)],
    );
}

/// Gauge: record `ClickHouse` store size metrics.
pub(crate) fn record_store_gauges(experiment_count: usize, activity_count: usize) {
    let meter = global::meter(TRACER);

    let g = meter.u64_gauge("clickhouse.store.experiments").build();
    g.record(experiment_count as u64, &[]);

    let g = meter.u64_gauge("clickhouse.store.activities").build();
    g.record(activity_count as u64, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_span_does_not_panic() {
        let _g = begin_connect("http://localhost:8123", "tumult");
        event_schema_initialized("tumult", 1);
    }

    #[test]
    fn ddl_event_does_not_panic() {
        let _g = begin_connect("http://localhost:8123", "tumult");
        event_ddl_executed("CREATE TABLE IF NOT EXISTS experiments ...");
    }

    #[test]
    fn gauges_do_not_panic() {
        record_store_gauges(10, 50);
    }
}
