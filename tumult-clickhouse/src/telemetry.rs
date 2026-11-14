//! `OTel` instrumentation for `ClickHouse` backend operations.

use opentelemetry::metrics::Meter;
use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};
use tumult_otel::SpanGuard;

const TRACER: &str = "tumult-clickhouse";

/// Recorded in place of a URL that does not parse. A string that cannot be
/// split into its parts cannot be shown to be free of credentials.
const UNPARSEABLE_URL: &str = "invalid-url";

/// The parts of a connection URL that are safe to export: where the server
/// is, never who connects to it.
struct Endpoint {
    /// Host without brackets, port or scheme.
    address: String,
    /// The written port, or the default of an `http`/`https` scheme.
    port: Option<u16>,
    /// `scheme://host[:port]`.
    redacted: String,
}

impl Endpoint {
    /// `None` when the URL does not parse or has no host.
    fn parse(raw: &str) -> Option<Self> {
        let url = url::Url::parse(raw).ok()?;
        let host = url.host_str()?;
        let address = match url.host()? {
            url::Host::Ipv6(ip) => ip.to_string(),
            _ => host.to_string(),
        };
        let redacted = match url.port() {
            Some(port) => format!("{}://{host}:{port}", url.scheme()),
            None => format!("{}://{host}", url.scheme()),
        };
        Some(Self {
            address,
            port: url.port_or_known_default(),
            redacted,
        })
    }
}

/// A connection URL reduced to `scheme://host[:port]`.
///
/// Userinfo, path, query and fragment are dropped: each of them can carry a
/// credential (`user:pass@`, `?password=`). A URL that does not parse becomes
/// a constant.
pub(crate) fn redacted_url(raw: &str) -> String {
    Endpoint::parse(raw).map_or_else(|| UNPARSEABLE_URL.to_string(), |e| e.redacted)
}

/// Span for `ClickHouse` connection + schema init.
pub(crate) fn begin_connect(url: &str, database: &str) -> SpanGuard {
    let tracer = global::tracer(TRACER);
    let mut attributes = vec![
        KeyValue::new("db.system", "clickhouse"),
        KeyValue::new("db.name", database.to_string()),
    ];
    // The raw URL may embed credentials, so only its parsed parts are recorded.
    if let Some(endpoint) = Endpoint::parse(url) {
        attributes.push(KeyValue::new("server.address", endpoint.address));
        if let Some(port) = endpoint.port {
            attributes.push(KeyValue::new("server.port", i64::from(port)));
        }
        attributes.push(KeyValue::new("db.connection_string", endpoint.redacted));
    } else {
        attributes.push(KeyValue::new("server.address", UNPARSEABLE_URL));
        attributes.push(KeyValue::new("db.connection_string", UNPARSEABLE_URL));
    }
    let span = tracer
        .span_builder("clickhouse.connect")
        .with_kind(SpanKind::Client)
        .with_attributes(attributes)
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

    fn connect_attributes(url: &str) -> Vec<KeyValue> {
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
            connect_attributes("https://admin:secret@clickhouse.internal:8443"),
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
            connect_attributes("http://ch:8123/?user=admin&password=hunter2"),
            endpoint("ch", 8123, "http://ch:8123")
        );
    }

    #[test]
    fn connect_span_drops_userinfo_path_query_and_fragment_together() {
        assert_eq!(
            connect_attributes("http://admin:s3cret@ch:8123/db?password=x#token=y"),
            endpoint("ch", 8123, "http://ch:8123")
        );
    }

    #[test]
    fn connect_span_keeps_the_host_when_a_query_value_contains_an_at_sign() {
        assert_eq!(
            connect_attributes("http://ch:8123?password=p@ss"),
            endpoint("ch", 8123, "http://ch:8123")
        );
    }

    #[test]
    fn connect_span_reports_the_scheme_default_port_when_none_is_written() {
        assert_eq!(
            connect_attributes("https://ch.example/"),
            endpoint("ch.example", 443, "https://ch.example")
        );
    }

    #[test]
    fn connect_span_records_a_constant_for_a_url_that_does_not_parse() {
        // An unescaped slash in the password: nothing after the scheme can be
        // told apart from the secret, so none of it is recorded.
        for url in ["http://admin:p/ss@ch:8123", "admin:hunter2@ch:8123", ""] {
            assert_eq!(
                connect_attributes(url),
                [
                    KeyValue::new("db.system", "clickhouse"),
                    KeyValue::new("db.name", "tumult"),
                    KeyValue::new("server.address", "invalid-url"),
                    KeyValue::new("db.connection_string", "invalid-url"),
                ],
                "{url}"
            );
        }
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
