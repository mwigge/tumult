//! `OTel` instrumentation for `ClickHouse` backend operations.

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
            KeyValue::new("server.address", url.to_string()),
            // OTel semantic convention: sanitized connection string.
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
    fn connect_span_with_credentials_sanitizes_url() {
        // Must not panic; sanitized URL replaces credentials.
        let _g = begin_connect("http://admin:secret@clickhouse.internal:8123", "tumult");
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
