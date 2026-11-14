//! `OTel` instrumentation for `ClickHouse` backend operations.

use opentelemetry::metrics::Meter;
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
    ];
    if let Some(port) = server.port {
        attributes.push(KeyValue::new("server.port", i64::from(port)));
    }
    // The connection string without userinfo, query and fragment.
    attributes.push(KeyValue::new("db.connection_string", sanitize_url(url)));
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
        format!("{}...", &statement[..statement.floor_char_boundary(128)])
    } else {
        statement.to_string()
    };
    cx.span().add_event(
        "clickhouse.ddl.executed",
        vec![KeyValue::new("db.statement", preview)],
    );
}

/// The meter the store records on outside tests.
pub(crate) fn meter() -> Meter {
    global::meter(TRACER)
}

/// Gauge: record `ClickHouse` store size metrics on `meter`.
pub(crate) fn record_store_gauges(meter: &Meter, experiment_count: usize, activity_count: usize) {
    let g = meter.u64_gauge("clickhouse.store.experiments").build();
    g.record(experiment_count as u64, &[]);

    let g = meter.u64_gauge("clickhouse.store.activities").build();
    g.record(activity_count as u64, &[]);
}

#[cfg(test)]
mod tests {
    use tumult_test_utils::{capture_spans, MetricCapture};

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
            // A password with an unescaped delimiter: the URL cannot be
            // read for certain, so none of it is recorded.
            (
                "http://admin:se/cret@clickhouse.internal",
                "[redacted-url]",
                None,
                "[redacted-url]",
            ),
            (
                "http://admin:se#cret@clickhouse.internal:8123/db",
                "[redacted-url]",
                None,
                "[redacted-url]",
            ),
            (
                "http://admin:se?cret@clickhouse.internal/db",
                "[redacted-url]",
                None,
                "[redacted-url]",
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

    /// The attributes of the span `begin_connect` exports for `url`.
    fn span_attributes(url: &str) -> Vec<KeyValue> {
        let ((), spans) = capture_spans(|| {
            let _span = begin_connect(url, "tumult");
        });
        assert_eq!(spans.len(), 1);
        spans[0].attributes.clone()
    }

    fn endpoint(address: &'static str, port: i64, connection: &'static str) -> Vec<KeyValue> {
        vec![
            KeyValue::new("db.system", "clickhouse"),
            KeyValue::new("db.name", "tumult"),
            KeyValue::new("server.address", address),
            KeyValue::new("server.port", port),
            KeyValue::new("db.connection_string", connection),
        ]
    }

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
            endpoint("localhost", 8123, "http://localhost:8123")
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
    fn connect_span_drops_userinfo() {
        assert_eq!(
            span_attributes("https://admin:secret@clickhouse.internal:8443"),
            endpoint(
                "clickhouse.internal",
                8443,
                "https://clickhouse.internal:8443"
            )
        );
    }

    #[test]
    fn connect_span_drops_credentials_passed_as_query_parameters() {
        assert_eq!(
            span_attributes("http://ch:8123/?user=admin&password=hunter2"),
            endpoint("ch", 8123, "http://ch:8123/")
        );
    }

    /// The path names no one, so it stays; what can carry a credential goes.
    #[test]
    fn connect_span_drops_userinfo_query_and_fragment_together() {
        assert_eq!(
            span_attributes("http://admin:s3cret@ch:8123/db?password=x#token=y"),
            endpoint("ch", 8123, "http://ch:8123/db")
        );
    }

    /// An unescaped `/`, `?` or `#` in the userinfo ends the authority early:
    /// a parser then reads the user name as the host and a numeric password
    /// prefix as the port. A literal `@` after the authority is the sign of
    /// that, so such a URL is recorded as a constant.
    #[test]
    fn connect_span_records_a_constant_when_an_at_sign_follows_the_authority() {
        for url in [
            "http://u:123/456@h",
            "http://admin:8123?x@ch",
            "http://admin:123#x@ch:8123",
            "http://admin/pw@ch:8123",
            "http://ch:8123?password=p@ss",
            "http://ch:8123/db#frag@ment",
        ] {
            assert_eq!(span_attributes(url), invalid_url(), "{url}");
        }
    }

    #[test]
    fn connect_span_keeps_the_host_when_an_escaped_at_sign_follows_the_authority() {
        assert_eq!(
            span_attributes("http://ch:8123/db?password=p%40ss"),
            endpoint("ch", 8123, "http://ch:8123/db")
        );
    }

    /// A single slash after the scheme leaves no authority to read the host
    /// from, so the userinfo cannot be told from it.
    #[test]
    fn connect_span_records_a_constant_for_a_single_slash_after_the_scheme() {
        assert_eq!(span_attributes("http:/u:pw@h"), invalid_url());
    }

    /// Only a port that is written is reported; none is assumed from the
    /// scheme.
    #[test]
    fn connect_span_reports_no_port_when_none_is_written() {
        assert_eq!(
            span_attributes("https://ch.example/"),
            [
                KeyValue::new("db.system", "clickhouse"),
                KeyValue::new("db.name", "tumult"),
                KeyValue::new("server.address", "ch.example"),
                KeyValue::new("db.connection_string", "https://ch.example/"),
            ]
        );
    }

    #[test]
    fn connect_span_records_a_constant_for_a_url_that_cannot_be_read() {
        // An unescaped slash in the password: nothing after the scheme can be
        // told apart from the secret, so none of it is recorded.
        assert_eq!(span_attributes("http://admin:p/ss@ch:8123"), invalid_url());
    }

    #[test]
    fn connect_span_drops_userinfo_from_a_url_without_a_scheme() {
        assert_eq!(
            span_attributes("admin:hunter2@ch:8123"),
            endpoint("ch", 8123, "ch:8123")
        );
    }

    fn invalid_url() -> Vec<KeyValue> {
        vec![
            KeyValue::new("db.system", "clickhouse"),
            KeyValue::new("db.name", "tumult"),
            KeyValue::new("server.address", "[redacted-url]"),
            KeyValue::new("db.connection_string", "[redacted-url]"),
        ]
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
    fn ddl_event_cuts_a_long_statement_on_a_character_boundary() {
        // Byte 128 falls inside the two-byte character that starts at 127.
        let statement = format!("{}{}", "x".repeat(127), "é".repeat(10));

        let recorded = ddl_statements_recorded(&[&statement]);

        assert_eq!(recorded, [format!("{}...", "x".repeat(127))]);
    }

    #[test]
    fn store_gauges_report_the_latest_counts() {
        let capture = MetricCapture::new();
        let meter = capture.meter(TRACER);
        record_store_gauges(&meter, 10, 50);
        record_store_gauges(&meter, 12, 61);

        let value =
            |name: &str| -> Vec<f64> { capture.points(name).iter().map(|p| p.value).collect() };
        assert_eq!(value("clickhouse.store.experiments"), [12.0]);
        assert_eq!(value("clickhouse.store.activities"), [61.0]);
    }
}
