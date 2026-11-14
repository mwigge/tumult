//! Compressed OTLP/gRPC exports. The Collector's `otlp` exporter compresses
//! every message with gzip unless it is told otherwise, and offers zstd;
//! the three OTLP services take both, and a message sent as it is.

use opentelemetry_proto::tonic::collector::logs::v1::logs_service_client::LogsServiceClient;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::metrics_service_client::MetricsServiceClient;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_client::TraceServiceClient;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::common::v1::AnyValue;
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::metrics::v1::number_data_point::Value as Number;
use opentelemetry_proto::tonic::metrics::v1::{
    metric::Data, Gauge, Metric, NumberDataPoint, ResourceMetrics, ScopeMetrics,
};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tumult_ingest::{grpc, IngestWriter};
use tumult_lake::Store;

/// 2026-03-31, well after the earliest time a span is taken to be real.
const TS: u64 = 1_774_980_000_000_000_000;

/// How a client sends its messages.
const ENCODINGS: [Option<CompressionEncoding>; 3] = [
    None,
    Some(CompressionEncoding::Gzip),
    Some(CompressionEncoding::Zstd),
];

struct Daemon {
    _dir: tempfile::TempDir,
    store: Store,
    channel: Channel,
    server: tokio::task::JoinHandle<Result<(), tonic::transport::Error>>,
}

impl Daemon {
    async fn start() -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("k.duckdb")).unwrap();
        let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 16);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let incoming = tonic::codegen::tokio_stream::wrappers::TcpListenerStream::new(listener);
        let server = tokio::spawn(grpc::router(ingest).serve_with_incoming(incoming));
        let channel = Channel::from_shared(format!("http://{addr}"))
            .unwrap()
            .connect()
            .await
            .unwrap();
        Self {
            _dir: dir,
            store,
            channel,
            server,
        }
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

#[tokio::test]
async fn a_trace_export_is_stored_whatever_its_compression() {
    let daemon = Daemon::start().await;
    for (sent, encoding) in (1..).zip(ENCODINGS) {
        let mut client = TraceServiceClient::new(daemon.channel.clone());
        if let Some(encoding) = encoding {
            client = client.send_compressed(encoding);
        }
        let answer = client.export(spans(&format!("span-{sent}"))).await;
        assert!(answer.is_ok(), "{encoding:?}: {answer:?}");
        assert_eq!(daemon.count("spans"), sent, "{encoding:?}");
    }
}

#[tokio::test]
async fn a_metrics_export_is_stored_whatever_its_compression() {
    let daemon = Daemon::start().await;
    for (sent, encoding) in (1..).zip(ENCODINGS) {
        let mut client = MetricsServiceClient::new(daemon.channel.clone());
        if let Some(encoding) = encoding {
            client = client.send_compressed(encoding);
        }
        let answer = client.export(gauge(&format!("gauge.{sent}"))).await;
        assert!(answer.is_ok(), "{encoding:?}: {answer:?}");
        assert_eq!(daemon.count("metric_gauges"), sent, "{encoding:?}");
    }
}

#[tokio::test]
async fn a_logs_export_is_stored_whatever_its_compression() {
    let daemon = Daemon::start().await;
    for (sent, encoding) in (1..).zip(ENCODINGS) {
        let mut client = LogsServiceClient::new(daemon.channel.clone());
        if let Some(encoding) = encoding {
            client = client.send_compressed(encoding);
        }
        let answer = client.export(log(&format!("line {sent}"))).await;
        assert!(answer.is_ok(), "{encoding:?}: {answer:?}");
        assert_eq!(daemon.count("logs"), sent, "{encoding:?}");
    }
}

/// A compressed message is bounded like any other: the limit is on what it
/// expands to, so a small message cannot make the daemon hold a large one.
#[tokio::test]
async fn a_compressed_message_is_measured_by_what_it_expands_to() {
    let daemon = Daemon::start().await;
    // Five mebibytes of one letter: a few kilobytes on the wire.
    let large = "x".repeat(5 * 1024 * 1024);
    for encoding in [CompressionEncoding::Gzip, CompressionEncoding::Zstd] {
        let mut client = LogsServiceClient::new(daemon.channel.clone()).send_compressed(encoding);
        let refused = client.export(log(&large)).await.unwrap_err();
        assert_eq!(
            refused.code(),
            tonic::Code::ResourceExhausted,
            "{encoding:?}: {refused}"
        );
    }
    assert_eq!(daemon.count("logs"), 0);
}

/// The same bound holds for a trace export: the name of one span that
/// expands to five mebibytes is refused, not held.
#[tokio::test]
async fn a_compressed_trace_export_is_measured_by_what_it_expands_to() {
    let daemon = Daemon::start().await;
    let large = "x".repeat(5 * 1024 * 1024);
    for encoding in [CompressionEncoding::Gzip, CompressionEncoding::Zstd] {
        let mut client = TraceServiceClient::new(daemon.channel.clone()).send_compressed(encoding);
        let refused = client.export(spans(&large)).await.unwrap_err();
        assert_eq!(
            refused.code(),
            tonic::Code::ResourceExhausted,
            "{encoding:?}: {refused}"
        );
    }
    assert_eq!(daemon.count("spans"), 0);
}

/// And for a metrics export.
#[tokio::test]
async fn a_compressed_metrics_export_is_measured_by_what_it_expands_to() {
    let daemon = Daemon::start().await;
    let large = "x".repeat(5 * 1024 * 1024);
    for encoding in [CompressionEncoding::Gzip, CompressionEncoding::Zstd] {
        let mut client =
            MetricsServiceClient::new(daemon.channel.clone()).send_compressed(encoding);
        let refused = client.export(gauge(&large)).await.unwrap_err();
        assert_eq!(
            refused.code(),
            tonic::Code::ResourceExhausted,
            "{encoding:?}: {refused}"
        );
    }
    assert_eq!(daemon.count("metric_gauges"), 0);
}
