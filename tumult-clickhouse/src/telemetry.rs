//! `OTel` instrumentation for `ClickHouse` backend operations.

use opentelemetry::metrics::Meter;
use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};
use tumult_otel::SpanGuard;

const TRACER: &str = "tumult-clickhouse";

/// Sanitize a `ClickHouse` URL by stripping credentials (user:pass@) before
/// recording it as a telemetry attribute.
///
/// Parses the authority portion `[user:pass@]host[:port]` and replaces
/// any embedded credentials with `****:****@` so the resulting string
/// is safe to export to an OTLP backend.
fn sanitize_connection_url(url: &str) -> String {
    // Strip the scheme if present (e.g. "http://", "https://", "clickhouse://").
    let after_scheme = url.find("://").map_or(url, |i| &url[i + 3..]);

    // Locate userinfo in the authority (ends at first '/' or end of string).
    let authority_end = after_scheme.find('/').unwrap_or(after_scheme.len());
    let authority = &after_scheme[..authority_end];

    if let Some(at_pos) = authority.rfind('@') {
        // Credentials present — replace them.
        let scheme_prefix = url.find("://").map_or("", |i| &url[..i + 3]);
        let host_part = &after_scheme[at_pos + 1..];
        format!("{scheme_prefix}****:****@{host_part}")
    } else {
        url.to_string()
    }
}

/// Span for `ClickHouse` connection + schema init.
pub(crate) fn begin_connect(url: &str, database: &str) -> SpanGuard {
    let tracer = global::tracer(TRACER);
    let safe_url = sanitize_connection_url(url);
    let span = tracer
        .span_builder("clickhouse.connect")
        .with_kind(SpanKind::Client)
        .with_attributes(vec![
            KeyValue::new("db.system", "clickhouse"),
            KeyValue::new("db.name", database.to_string()),
            // Both carry the sanitized form: the raw URL may embed credentials.
            KeyValue::new("server.address", safe_url.clone()),
            KeyValue::new("db.connection_string", safe_url),
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
    record_store_gauges_on(&global::meter(TRACER), experiment_count, activity_count);
}

/// [`record_store_gauges`] against an explicit meter, so the recorded values
/// can be read back without a process-wide meter provider.
fn record_store_gauges_on(meter: &Meter, experiment_count: usize, activity_count: usize) {
    let g = meter.u64_gauge("clickhouse.store.experiments").build();
    g.record(experiment_count as u64, &[]);

    let g = meter.u64_gauge("clickhouse.store.activities").build();
    g.record(activity_count as u64, &[]);
}

#[cfg(test)]
mod tests {
    use tumult_test_utils::{capture_spans, MetricCapture};

    use super::*;

    #[test]
    fn connect_span_describes_the_server_and_records_schema_initialisation() {
        let ((), spans) = capture_spans(|| {
            let _span = begin_connect("http://localhost:8123", "tumult");
            event_schema_initialized("tumult", 1);
        });

        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.name, "clickhouse.connect");
        assert_eq!(span.span_kind, SpanKind::Client);
        assert_eq!(
            span.attributes,
            [
                KeyValue::new("db.system", "clickhouse"),
                KeyValue::new("db.name", "tumult"),
                KeyValue::new("server.address", "http://localhost:8123"),
                KeyValue::new("db.connection_string", "http://localhost:8123"),
            ]
        );
        let events = &span.events.events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "clickhouse.schema.initialized");
        assert_eq!(
            events[0].attributes,
            [
                KeyValue::new("db.name", "tumult"),
                KeyValue::new("db.schema.version", 1_i64),
            ]
        );
    }

    #[test]
    fn connect_span_never_carries_the_credentials_of_the_url() {
        let ((), spans) = capture_spans(|| {
            let _span = begin_connect("http://admin:secret@clickhouse.internal:8123", "tumult");
        });

        let sanitized = "http://****:****@clickhouse.internal:8123";
        assert_eq!(
            spans[0].attributes,
            [
                KeyValue::new("db.system", "clickhouse"),
                KeyValue::new("db.name", "tumult"),
                KeyValue::new("server.address", sanitized),
                KeyValue::new("db.connection_string", sanitized),
            ]
        );
    }

    #[test]
    fn sanitize_strips_user_and_password() {
        let result = sanitize_connection_url("http://user:pass@host:8123/db");
        assert_eq!(result, "http://****:****@host:8123/db");
    }

    #[test]
    fn sanitize_no_credentials_unchanged() {
        let result = sanitize_connection_url("http://host:8123/db");
        assert_eq!(result, "http://host:8123/db");
    }

    #[test]
    fn sanitize_bare_host_unchanged() {
        let result = sanitize_connection_url("http://localhost:8123");
        assert_eq!(result, "http://localhost:8123");
    }

    #[test]
    fn sanitize_preserves_path_and_port() {
        let result = sanitize_connection_url("clickhouse://admin:password@ch.prod:9000/tumult");
        assert_eq!(result, "clickhouse://****:****@ch.prod:9000/tumult");
    }

    fn ddl_statements_recorded(statements: &[&str]) -> Vec<String> {
        let ((), spans) = capture_spans(|| {
            let _span = begin_connect("http://localhost:8123", "tumult");
            for statement in statements {
                event_ddl_executed(statement);
            }
        });
        spans[0]
            .events
            .events
            .iter()
            .map(|event| {
                assert_eq!(event.name, "clickhouse.ddl.executed");
                assert_eq!(event.attributes.len(), 1);
                assert_eq!(event.attributes[0].key.as_str(), "db.statement");
                event.attributes[0].value.as_str().into_owned()
            })
            .collect()
    }

    #[test]
    fn ddl_event_records_the_statement_cut_to_128_bytes() {
        let exact = "x".repeat(128);
        let long = "y".repeat(129);

        let recorded = ddl_statements_recorded(&["CREATE DATABASE `tumult`", &exact, &long]);

        assert_eq!(
            recorded,
            [
                "CREATE DATABASE `tumult`".to_string(),
                exact,
                format!("{}...", "y".repeat(128)),
            ]
        );
    }

    #[test]
    fn store_gauges_report_the_latest_counts() {
        let capture = MetricCapture::new();
        let meter = capture.meter(TRACER);
        record_store_gauges_on(&meter, 10, 50);
        record_store_gauges_on(&meter, 12, 61);

        let value =
            |name: &str| -> Vec<f64> { capture.points(name).iter().map(|p| p.value).collect() };
        assert_eq!(value("clickhouse.store.experiments"), [12.0]);
        assert_eq!(value("clickhouse.store.activities"), [61.0]);
    }
}
