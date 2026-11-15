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

/// A gzip body may be several members one after the other (RFC 1952); it
/// is what they expand to together, so nothing of it is left unread.
#[tokio::test]
async fn a_gzip_body_of_two_members_is_stored_whole() {
    let daemon = Daemon::start().await;
    for (route, table) in ROUTES {
        let body = [gzip(&export(route, "one")), gzip(&export(route, "two"))].concat();
        let status = daemon.post(route, Some("gzip"), body).await;
        assert_eq!(status, 200, "{route}");
        assert_eq!(daemon.count(table), 2, "{route}");
    }
}

/// The members are bounded together: two of three mebibytes each are
/// inside the bound one by one and too large as a body.
#[tokio::test]
async fn the_members_of_a_gzip_body_are_measured_together() {
    let daemon = Daemon::start().await;
    let large = "x".repeat(3 * 1024 * 1024);
    for (route, table) in ROUTES {
        let member = gzip(&export(route, &large));
        let status = daemon.post(route, Some("gzip"), member.clone()).await;
        assert_eq!(status, 200, "{route}: one member");
        let status = daemon.post(route, Some("gzip"), member.repeat(2)).await;
        assert_eq!(status, 413, "{route}: two members");
        assert_eq!(daemon.count(table), 1, "{route}");
    }
}

/// What follows a member and is not one makes the body malformed; the
/// member before it is not stored.
#[tokio::test]
async fn a_gzip_member_followed_by_something_else_is_a_client_error() {
    let daemon = Daemon::start().await;
    for (route, table) in ROUTES {
        let mut body = gzip(&export(route, "one"));
        body.extend_from_slice(b"not a gzip member");
        let status = daemon.post(route, Some("gzip"), body).await;
        assert_eq!(status, 400, "{route}");
        assert_eq!(daemon.count(table), 0, "{route}");
    }
}

/// The status the router gives `body` on `route`, without a socket: a
/// refusal that comes before the body is read is seen as it is.
async fn answer(route: &str, encoding: Option<&str>, body: Vec<u8>) -> (u16, tempfile::TempDir) {
    use tower::ServiceExt;

    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open(&dir.path().join("k.duckdb")).unwrap();
    let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 16);
    let mut request =
        axum::http::Request::post(route).header("content-type", "application/x-protobuf");
    if let Some(encoding) = encoding {
        request = request.header("content-encoding", encoding);
    }
    let request = request.body(axum::body::Body::from(body)).unwrap();
    let response = http::router(ingest).oneshot(request).await.unwrap();
    (response.status().as_u16(), dir)
}

/// An export for `route` of exactly `len` bytes.
fn export_of(route: &str, len: usize) -> Vec<u8> {
    let framing = export(route, &"x".repeat(len - 64)).len() - (len - 64);
    let body = export(route, &"x".repeat(len - framing));
    assert_eq!(body.len(), len, "{route}");
    body
}

/// Bytes gzip cannot make smaller (a xorshift sequence).
fn noise(len: usize) -> Vec<u8> {
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state.to_le_bytes()[0]
    };
    (0..len).map(|_| next()).collect()
}

/// The body on the wire is bounded whatever its encoding: one of exactly
/// the bound is read, one byte more is too large.
#[tokio::test]
async fn a_body_larger_than_the_bound_on_the_wire_is_too_large() {
    for (route, _) in ROUTES {
        let at_the_bound = export_of(route, http::MAX_BODY_BYTES);
        let (status, _dir) = answer(route, None, at_the_bound).await;
        assert_eq!(status, 200, "{route}: exactly the bound");
        let one_more = export_of(route, http::MAX_BODY_BYTES + 1);
        let (status, _dir) = answer(route, None, one_more).await;
        assert_eq!(status, 413, "{route}: one byte more");
    }
}

/// A body sent as it is may be as large as a gzip body may expand to, and
/// as a message may be on gRPC: 4 MiB, one bound whatever the encoding.
#[tokio::test]
async fn a_plain_body_may_be_as_large_as_a_gzip_body_may_expand_to() {
    assert_eq!(http::MAX_BODY_BYTES, 4 * 1024 * 1024);
    for (route, _) in ROUTES {
        let plain = export_of(route, 3 * 1024 * 1024);
        let (status, _dir) = answer(route, None, plain.clone()).await;
        assert_eq!(status, 200, "{route}: as it is");
        let (status, _dir) = answer(route, Some("gzip"), gzip(&plain)).await;
        assert_eq!(status, 200, "{route}: gzip");
    }
}

/// A gzip body is bounded on the wire too, before anything is expanded.
#[tokio::test]
async fn a_gzip_body_larger_than_the_bound_on_the_wire_is_too_large() {
    let body = gzip(&noise(5 * 1024 * 1024));
    assert!(body.len() > http::MAX_BODY_BYTES, "{} bytes", body.len());
    for (route, _) in ROUTES {
        let (status, _dir) = answer(route, Some("gzip"), body.clone()).await;
        assert_eq!(status, 413, "{route}");
    }
}
