//! `OTel` instrumentation for `ClickHouse` backend operations.

use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};
use tumult_otel::logging::{sanitize_url, server_address};
use tumult_otel::SpanGuard;

const TRACER: &str = "tumult-clickhouse";

/// Span for `ClickHouse` connection + schema init.
pub(crate) fn begin_connect(url: &str, database: &str) -> SpanGuard {
    let tracer = global::tracer(TRACER);
    let span = tracer
        .span_builder("clickhouse.connect")
        .with_kind(SpanKind::Client)
        .with_attributes(connect_attributes(url, database))
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    SpanGuard::new(cx.attach())
}

/// The attributes of the connect span. `url` is configuration and may carry
/// credentials, so it is never recorded as given.
fn connect_attributes(url: &str, database: &str) -> Vec<KeyValue> {
    let server = server_address(url);
    let mut attributes = vec![
        KeyValue::new("db.system", "clickhouse"),
        KeyValue::new("db.name", database.to_string()),
        KeyValue::new("server.address", server.host),
        // The connection string without userinfo, query and fragment.
        KeyValue::new("db.connection_string", sanitize_url(url)),
    ];
    if let Some(port) = server.port {
        attributes.push(KeyValue::new("server.port", i64::from(port)));
    }
    attributes
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

    fn attribute(attributes: &[KeyValue], key: &str) -> Option<String> {
        attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == key)
            .map(|attribute| attribute.value.to_string())
    }

    #[test]
    fn the_connect_span_names_the_host_and_port_and_no_credentials() {
        // (configured URL, server.address, server.port, db.connection_string)
        let cases = [
            (
                "http://admin:secret@clickhouse.internal:8123/db?password=secret",
                "clickhouse.internal",
                Some("8123"),
                "http://clickhouse.internal:8123/db",
            ),
            (
                "http://admin:se/cret@clickhouse.internal",
                "clickhouse.internal",
                None,
                "http://clickhouse.internal",
            ),
            (
                "clickhouse://admin:secret@ch.prod:9000/tumult",
                "ch.prod",
                Some("9000"),
                "clickhouse://ch.prod:9000/tumult",
            ),
            (
                "https://[::1]:8443",
                "::1",
                Some("8443"),
                "https://[::1]:8443",
            ),
            (
                "http://localhost:8123",
                "localhost",
                Some("8123"),
                "http://localhost:8123",
            ),
        ];
        for (url, host, port, connection) in cases {
            let attributes = connect_attributes(url, "tumult");
            assert_eq!(
                attribute(&attributes, "server.address").as_deref(),
                Some(host),
                "{url}"
            );
            assert_eq!(
                attribute(&attributes, "server.port").as_deref(),
                port,
                "{url}"
            );
            assert_eq!(
                attribute(&attributes, "db.connection_string").as_deref(),
                Some(connection),
                "{url}"
            );
            for recorded in &attributes {
                let value = recorded.value.to_string();
                for secret in ["admin", "cret"] {
                    assert!(
                        !value.contains(secret),
                        "{} leaks {secret} from {url}: {value}",
                        recorded.key
                    );
                }
            }
        }
    }

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
    fn ddl_event_does_not_panic() {
        let _g = begin_connect("http://localhost:8123", "tumult");
        event_ddl_executed("CREATE TABLE IF NOT EXISTS experiments ...");
    }

    #[test]
    fn gauges_do_not_panic() {
        record_store_gauges(10, 50);
    }
}
