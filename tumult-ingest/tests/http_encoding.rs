//! `Content-Encoding` on the OTLP/HTTP routes. The Collector's `otlphttp`
//! exporter compresses every body with gzip unless it is told otherwise;
//! the three routes take gzip and a body sent as it is, bound what a
//! compressed body may expand to, and refuse every other encoding.

use std::io::Write;

use flate2::write::GzEncoder;
use flate2::Compression;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::AnyValue;
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::number_data_point::Value as Number;
use opentelemetry_proto::tonic::metrics::v1::{
    metric::Data, Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics,
};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use tumult_ingest::{http, IngestWriter};
use tumult_lake::Store;

/// 2026-03-31, well after the earliest time a span is taken to be real.
const TS: u64 = 1_774_980_000_000_000_000;

/// The routes with the table each one fills.
const ROUTES: [(&str, &str); 3] = [
    ("/v1/traces", "spans"),
    ("/v1/metrics", "metric_gauges"),
    ("/v1/logs", "logs"),
];

struct Daemon {
    _dir: tempfile::TempDir,
    store: Store,
    base: String,
    server: tokio::task::JoinHandle<()>,
}

impl Daemon {
    async fn start() -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("k.duckdb")).unwrap();
        let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 16);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, http::router(ingest)).await.unwrap();
        });
        Self {
            _dir: dir,
            store,
            base,
            server,
        }
    }

    /// Post `body` to `route`, said to be encoded as `encoding`; the
    /// status of the answer.
    async fn post(&self, route: &str, encoding: Option<&str>, body: Vec<u8>) -> u16 {
        let mut request = reqwest::Client::new()
            .post(format!("{}{route}", self.base))
            .header("content-type", "application/x-protobuf");
        if let Some(encoding) = encoding {
            request = request.header("content-encoding", encoding);
        }
        request.body(body).send().await.unwrap().status().as_u16()
    }

    fn count(&self, table: &str) -> u64 {
        let rows = self
            .store
            .read_only()
            .unwrap()
            .query_json_rows(&format!("SELECT count(*) AS n FROM {table}"))
            .unwrap();
        rows[0]["n"].as_u64().unwrap()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn gzip(plain: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(plain).unwrap();
    encoder.finish().unwrap()
}

/// An export for `route` that carries `text` once: as the name of a span,
/// the name of a gauge or the body of a log record.
fn export(route: &str, text: &str) -> Vec<u8> {
    match route {
        "/v1/traces" => spans(text).encode_to_vec(),
        "/v1/metrics" => gauge(text).encode_to_vec(),
        _ => log(text).encode_to_vec(),
    }
}

fn spans(name: &str) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans: vec![Span {
                    trace_id: vec![0xab; 16],
                    span_id: vec![0xcd; 8],
                    name: name.into(),
                    start_time_unix_nano: TS,
                    end_time_unix_nano: TS + 1_000_000,
                    ..Span::default()
                }],
                ..ScopeSpans::default()
            }],
            ..ResourceSpans::default()
        }],
    }
}

fn gauge(name: &str) -> ExportMetricsServiceRequest {
    ExportMetricsServiceRequest {
        resource_metrics: vec![ResourceMetrics {
            scope_metrics: vec![ScopeMetrics {
                metrics: vec![Metric {
                    name: name.into(),
                    data: Some(Data::Gauge(Gauge {
                        data_points: vec![NumberDataPoint {
                            time_unix_nano: TS,
                            value: Some(Number::AsInt(7)),
                            ..NumberDataPoint::default()
                        }],
                    })),
                    ..Metric::default()
                }],
                ..ScopeMetrics::default()
            }],
            ..ResourceMetrics::default()
        }],
    }
}

fn log(body: &str) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: vec![LogRecord {
                    time_unix_nano: TS,
                    body: Some(AnyValue {
                        value: Some(Value::StringValue(body.into())),
                    }),
                    ..LogRecord::default()
                }],
                ..ScopeLogs::default()
            }],
            ..ResourceLogs::default()
        }],
    }
}

/// What the Collector sends by default is stored, on every route; so is a
/// body that says it is not encoded, and the name of the encoding is read
/// without regard to case.
#[tokio::test]
async fn a_gzip_body_is_stored_on_every_route() {
    let daemon = Daemon::start().await;
    for (route, table) in ROUTES {
        let plain = export(route, "one");
        let status = daemon.post(route, Some("gzip"), gzip(&plain)).await;
        assert_eq!(status, 200, "{route}");
        assert_eq!(daemon.count(table), 1, "{route}");

        let status = daemon.post(route, Some("GZip"), gzip(&plain)).await;
        assert_eq!(status, 200, "{route}: the name in another case");
        let status = daemon.post(route, Some("identity"), plain.clone()).await;
        assert_eq!(status, 200, "{route}: said to be as it is");
        let status = daemon.post(route, None, plain).await;
        assert_eq!(status, 200, "{route}: no encoding named");
        assert_eq!(daemon.count(table), 4, "{route}");
    }
}

/// A compressed body is bounded by what it expands to: five mebibytes of
/// one letter are a few kilobytes on the wire and are refused as too
/// large, with nothing stored.
#[tokio::test]
async fn a_gzip_body_is_measured_by_what_it_expands_to() {
    let daemon = Daemon::start().await;
    let large = "x".repeat(5 * 1024 * 1024);
    for (route, table) in ROUTES {
        let bomb = gzip(&export(route, &large));
        assert!(bomb.len() < 64 * 1024, "{route}: {} bytes", bomb.len());
        let status = daemon.post(route, Some("gzip"), bomb).await;
        assert_eq!(status, 413, "{route}");
        assert_eq!(daemon.count(table), 0, "{route}");
    }
}

/// An encoding the daemon does not read is said to be unsupported: never
/// accepted (the body would be lost) and never called malformed (the
/// sender would not know to change its compression).
#[tokio::test]
async fn any_other_encoding_is_refused_as_unsupported() {
    let daemon = Daemon::start().await;
    for (route, table) in ROUTES {
        let plain = export(route, "one");
        for encoding in ["zstd", "br", "deflate", "snappy", "gzip, gzip", "x"] {
            let status = daemon.post(route, Some(encoding), plain.clone()).await;
            assert_eq!(status, 415, "{route} {encoding}");
        }
        assert_eq!(daemon.count(table), 0, "{route}");
    }
}

/// A body that says gzip and is not is the sender's error.
#[tokio::test]
async fn a_body_that_is_not_gzip_is_a_client_error() {
    let daemon = Daemon::start().await;
    for (route, table) in ROUTES {
        let plain = export(route, "one");
        let status = daemon.post(route, Some("gzip"), plain.clone()).await;
        assert_eq!(status, 400, "{route}: not compressed at all");
        let mut cut = gzip(&plain);
        cut.truncate(cut.len() - 6);
        let status = daemon.post(route, Some("gzip"), cut).await;
        assert_eq!(status, 400, "{route}: a stream that stops early");
        assert_eq!(daemon.count(table), 0, "{route}");
    }
}
