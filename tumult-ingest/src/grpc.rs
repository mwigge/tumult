//! OTLP/gRPC services (what tumult's exporter talks: bare `host:4317`),
//! plus the OTel-Arrow stream services ([`crate::otap`]) on the same
//! listener.
//!
//! When an ingest token is configured (`KRONIKA_INGEST_TOKEN`), every
//! export call (and every Arrow stream) must carry an
//! `authorization: Bearer <token>` metadata entry; anything else is rejected
//! with `UNAUTHENTICATED`. The token is checked on the headers of a call
//! ([`bearer`]), before its message is read or decoded. The first
//! authorised call also tells the listener ([`crate::listener`]) that the
//! connection belongs to a client: until then it has a deadline and gives
//! way to a newcomer when the listener is full.
//!
//! TLS is the listener's business ([`crate::listener::Incoming::with_tls`]),
//! not the router's: the listener must know when a handshake is complete.

use opentelemetry_proto::tonic::collector::logs::v1::logs_service_server::{
    LogsService, LogsServiceServer,
};
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use opentelemetry_proto::tonic::collector::metrics::v1::metrics_service_server::{
    MetricsService, MetricsServiceServer,
};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_server::{
    TraceService, TraceServiceServer,
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::server::Router;
use tonic::{Request, Response, Status};

use crate::writer::{Batch, IngestWriter};

/// HTTP/2 streams one connection may have open at once. A collector uses a
/// handful (one Arrow stream per signal and exporter worker, plus unary OTLP
/// calls); the cap stops one peer from opening streams without bound.
pub const MAX_CONCURRENT_STREAMS_PER_CONNECTION: u32 = 64;

/// Requests one connection may have in flight at once; further ones wait
/// (back-pressure) instead of piling up work.
pub const MAX_CONCURRENT_REQUESTS_PER_CONNECTION: usize = 64;

/// How often an HTTP/2 keepalive ping is sent on a connection.
pub const HTTP2_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(30);

/// How long a keepalive ping may go unanswered before the connection is
/// dropped, so a vanished peer does not hold its streams (and their slots in
/// [`crate::otap::OtapLimits::max_streams`]) forever.
pub const HTTP2_KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(20);

/// TCP keepalive on accepted sockets, for peers that died before HTTP/2 was
/// established. tonic applies its own `tcp_keepalive` setting only to a
/// listener it creates, so [`crate::listener`] sets this on every socket it
/// accepts.
pub const TCP_KEEPALIVE: Duration = Duration::from_secs(60);

/// Build the tonic gRPC router for the three OTLP collector services,
/// unauthenticated.
pub fn router(ingest: IngestWriter) -> Router {
    router_with_token(ingest, None)
}

/// Build the tonic gRPC router; when `ingest_token` is `Some`, every export
/// call requires an `authorization: Bearer <token>` metadata entry.
pub fn router_with_token(ingest: IngestWriter, ingest_token: Option<String>) -> Router {
    // Arrow stream limits (rows, attributes, decoded bytes, message size,
    // open streams, idle time) and the connection time limits come from
    // the environment, see `OtapLimits` and `ConnectionLimits`.
    build(
        ingest,
        ingest_token,
        crate::otap::OtapLimits::from_env(),
        crate::listener::ConnectionLimits::from_env(),
    )
}

/// [`router_with_token`] with the limits given.
fn build(
    ingest: IngestWriter,
    ingest_token: Option<String>,
    otap_limits: crate::otap::OtapLimits,
    connections: crate::listener::ConnectionLimits,
) -> Router {
    let token = ingest_token.map(std::sync::Arc::new);
    // A connection is dropped when its age and grace are over; the
    // listener (`listener::LimitedStream`) enforces the same limit on the
    // socket itself. Both settings must be given to tonic 0.14: with the
    // age alone its connection task polls a finished timer a second time
    // and panics, which aborts a release build.
    let mut builder = tonic::transport::Server::builder()
        .max_concurrent_streams(MAX_CONCURRENT_STREAMS_PER_CONNECTION)
        .concurrency_limit_per_connection(MAX_CONCURRENT_REQUESTS_PER_CONNECTION)
        .http2_keepalive_interval(Some(HTTP2_KEEPALIVE_INTERVAL))
        .http2_keepalive_timeout(Some(HTTP2_KEEPALIVE_TIMEOUT))
        .max_connection_age(connections.max_connection_age)
        .max_connection_age_grace(connections.max_connection_age_grace);
    let otap =
        crate::otap::service::OtapGrpc::with_limits(ingest.clone(), token.clone(), otap_limits);
    let otlp = OtlpGrpc {
        ingest,
        ingest_token: token.clone(),
    };
    // The Collector's `otlp` exporter compresses with gzip unless told
    // otherwise and offers zstd; both are taken, as on the OTel-Arrow
    // services. The message size limit is on what a message expands to.
    let traces = TraceServiceServer::new(otlp.clone())
        .accept_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Zstd);
    let metrics = MetricsServiceServer::new(otlp.clone())
        .accept_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Zstd);
    let logs = LogsServiceServer::new(otlp)
        .accept_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Zstd);
    builder
        .add_service(InterceptedService::new(traces, bearer(token.clone())))
        .add_service(InterceptedService::new(metrics, bearer(token.clone())))
        .add_service(InterceptedService::new(logs, bearer(token)))
        // OTel-Arrow: same listener, same token, same writer channel.
        .add_service(otap.clone().traces())
        .add_service(otap.clone().logs())
        .add_service(otap.metrics())
}

/// Bearer-token check for one incoming call: `None` token accepts
/// everything; otherwise the `authorization` metadata must be exactly
/// `Bearer <token>` (constant-time compare). A rejection is logged at WARN
/// (rate-limited, with a count of what was left out), saying only whether
/// a credential was presented, never what it was.
///
/// An accepted call marks its connection as a client's
/// ([`crate::listener::authenticated`]).
pub(crate) fn authorize<T>(
    ingest_token: Option<&std::sync::Arc<String>>,
    request: &Request<T>,
) -> Result<(), Status> {
    let Some(token) = ingest_token else {
        crate::listener::authenticated(request);
        return Ok(());
    };
    let expected = format!("Bearer {token}");
    let presented = request
        .metadata()
        .get("authorization")
        .and_then(|v| v.to_str().ok());
    match presented {
        Some(v) if tumult_auth::constant_time_eq(v, &expected) => {
            crate::listener::authenticated(request);
            Ok(())
        }
        _ => {
            let peer = crate::listener::remote_addr(request).map(|peer| peer.ip());
            crate::log_guard::warn_rejected("grpc", presented.is_some(), peer);
            Err(Status::unauthenticated("missing or invalid bearer token"))
        }
    }
}

/// [`authorize`] as an interceptor: it sees the headers of a call and
/// answers before the body is read, so a caller without the token cannot
/// make the receiver buffer and decode a message. (An Arrow stream is
/// checked the same way when it opens: its messages are read lazily.)
fn bearer(
    ingest_token: Option<std::sync::Arc<String>>,
) -> impl tonic::service::Interceptor + Clone {
    move |request: Request<()>| {
        authorize(ingest_token.as_ref(), &request)?;
        Ok(request)
    }
}

/// The unary OTLP services. Each call checks the token again: the
/// interceptor is set where the router is built, the check here holds
/// wherever the service is used.
#[derive(Clone)]
struct OtlpGrpc {
    ingest: IngestWriter,
    ingest_token: Option<std::sync::Arc<String>>,
}

fn to_status(e: crate::error::IngestError) -> Status {
    tracing::error!(error = %e, "OTLP write failed");
    Status::internal("telemetry write failed")
}

#[tonic::async_trait]
impl TraceService for OtlpGrpc {
    async fn export(
        &self,
        request: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        authorize(self.ingest_token.as_ref(), &request)?;
        let spans = tumult_otlp::trace_request_to_spans(request.get_ref());
        self.ingest
            .write(Batch::Spans(spans))
            .await
            .map_err(to_status)?;
        Ok(Response::new(ExportTraceServiceResponse {
            partial_success: None,
        }))
    }
}

#[tonic::async_trait]
impl MetricsService for OtlpGrpc {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        authorize(self.ingest_token.as_ref(), &request)?;
        let rows = tumult_otlp::metrics_request_to_rows(request.get_ref());
        let response = crate::http::metrics_response(rows.rejected_data_points);
        self.ingest
            .write(Batch::Metrics(rows))
            .await
            .map_err(to_status)?;
        Ok(Response::new(response))
    }
}

#[tonic::async_trait]
impl LogsService for OtlpGrpc {
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        authorize(self.ingest_token.as_ref(), &request)?;
        let answer = self.ingest.export_logs(request.get_ref()).await;
        answer.map(Response::new).map_err(to_status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::metadata::MetadataValue;

    fn token(value: &str) -> Option<std::sync::Arc<String>> {
        Some(std::sync::Arc::new(value.to_string()))
    }

    #[test]
    fn no_token_accepts_any_metadata() {
        let req = Request::new(());
        assert!(authorize(None, &req).is_ok());
    }

    #[test]
    fn correct_bearer_is_accepted() {
        let mut req = Request::new(());
        req.metadata_mut().insert(
            "authorization",
            MetadataValue::from_static("Bearer kro_secret"),
        );
        assert!(authorize(token("kro_secret").as_ref(), &req).is_ok());
    }

    #[test]
    fn missing_or_wrong_bearer_is_unauthenticated() {
        let missing = Request::new(());
        let err = authorize(token("kro_secret").as_ref(), &missing).unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);

        let mut wrong = Request::new(());
        wrong.metadata_mut().insert(
            "authorization",
            MetadataValue::from_static("Bearer kro_other"),
        );
        let err = authorize(token("kro_secret").as_ref(), &wrong).unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);

        let mut no_scheme = Request::new(());
        no_scheme
            .metadata_mut()
            .insert("authorization", MetadataValue::from_static("kro_secret"));
        let err = authorize(token("kro_secret").as_ref(), &no_scheme).unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    #[test]
    fn rejected_tokens_are_logged_at_warn_without_their_contents() {
        let (logs, _guard) = crate::test_logs::capture();
        let mut wrong = Request::new(());
        wrong.metadata_mut().insert(
            "authorization",
            MetadataValue::from_static("Bearer kro_other"),
        );
        authorize(token("kro_secret").as_ref(), &wrong).unwrap_err();
        authorize(token("kro_secret").as_ref(), &Request::new(())).unwrap_err();
        let text = logs.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        assert!(
            lines[0].contains("WARN") && lines[0].contains("presented=true"),
            "{text}"
        );
        assert!(
            lines[1].contains("WARN") && lines[1].contains("presented=false"),
            "{text}"
        );
        assert!(text.contains("ingest bearer token rejected"), "{text}");
        assert!(
            !text.contains("kro_other") && !text.contains("kro_secret"),
            "{text}"
        );

        // An accepted call logs nothing.
        let (logs, _guard) = crate::test_logs::capture();
        let mut right = Request::new(());
        right.metadata_mut().insert(
            "authorization",
            MetadataValue::from_static("Bearer kro_secret"),
        );
        authorize(token("kro_secret").as_ref(), &right).unwrap();
        assert_eq!(logs.text(), "");
    }

    #[tokio::test]
    async fn only_an_authorised_call_marks_its_connection_authenticated() {
        use crate::listener::{Access, ConnectionLimits, Gate};
        use tonic::transport::server::Connected;

        let gate = Gate::new(ConnectionLimits::default(), Access::Token);
        let peer = crate::listener::testing::from("192.0.2.1");
        // Three connections the listener let in, and a call on each.
        let connections: Vec<_> = (0..3)
            .map(|_| gate.admit(tokio::io::empty(), peer).unwrap())
            .collect();
        let call = |connection: usize, bearer: Option<&'static str>| {
            let mut request = Request::new(());
            let info = connections[connection].connect_info();
            request.extensions_mut().insert(info);
            if let Some(bearer) = bearer {
                request
                    .metadata_mut()
                    .insert("authorization", MetadataValue::from_static(bearer));
            }
            request
        };
        let secret = token("kro_secret");
        authorize(secret.as_ref(), &call(0, Some("Bearer kro_other"))).unwrap_err();
        authorize(secret.as_ref(), &call(1, None)).unwrap_err();
        assert_eq!((gate.open(), gate.unauthenticated()), (3, 3));

        authorize(secret.as_ref(), &call(2, Some("Bearer kro_secret"))).unwrap();
        assert_eq!((gate.open(), gate.unauthenticated()), (3, 2));
    }

    #[test]
    fn a_flood_of_rejected_tokens_does_not_flood_the_log() {
        let (logs, _guard) = crate::test_logs::capture();
        for _ in 0..500 {
            authorize(token("kro_secret").as_ref(), &Request::new(())).unwrap_err();
        }
        let lines = logs.text().lines().count();
        assert!(lines <= 20, "{lines} log lines for 500 rejected calls");
    }

    fn test_ingest() -> (tempfile::TempDir, tumult_lake::Store, IngestWriter) {
        let d = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&d.path().join("k.duckdb")).unwrap();
        let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 16);
        (d, store, ingest)
    }

    /// The token is looked at before the message: someone without it does
    /// not get the receiver to read and decode megabytes, and the warning
    /// says where the call came from.
    #[tokio::test]
    async fn a_unary_call_without_the_token_is_refused_before_its_message_is_decoded() {
        use opentelemetry_proto::tonic::collector::trace::v1::trace_service_client::TraceServiceClient;
        use opentelemetry_proto::tonic::trace::v1::ResourceSpans;

        let router = router_with_token(IngestWriter::stopped_for_test(), Some("kro_secret".into()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let incoming = tonic::codegen::tokio_stream::wrappers::TcpListenerStream::new(listener);
        let server = tokio::spawn(router.serve_with_incoming(incoming));
        let mut client = TraceServiceClient::connect(format!("http://{addr}"))
            .await
            .unwrap();

        // More than the server decodes (4 MiB): had the message been
        // looked at, the answer would be about its size.
        let too_large = ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                schema_url: "x".repeat(5 * 1024 * 1024),
                ..ResourceSpans::default()
            }],
        };
        let with_token = |bearer: &'static str| {
            let mut request = Request::new(too_large.clone());
            request
                .metadata_mut()
                .insert("authorization", MetadataValue::from_static(bearer));
            request
        };
        let (logs, _guard) = crate::test_logs::capture();
        let refused = client.export(with_token("Bearer kro_other")).await;
        let refused = refused.unwrap_err();
        assert_eq!(refused.code(), tonic::Code::Unauthenticated, "{refused}");
        let refused = client.export(Request::new(too_large.clone())).await;
        let refused = refused.unwrap_err();
        assert_eq!(refused.code(), tonic::Code::Unauthenticated, "{refused}");
        let text = logs.text();
        assert_eq!(text.matches(" peer=127.0.0.1 ").count(), 2, "{text}");
        assert_eq!(text.matches("carrier=\"grpc\"").count(), 2, "{text}");

        // With the token the message is read, and its size is the answer.
        let read = client.export(with_token("Bearer kro_secret")).await;
        let read = read.unwrap_err();
        assert_eq!(read.code(), tonic::Code::OutOfRange, "{read}");
        server.abort();
    }

    #[tokio::test]
    async fn routers_build_with_and_without_token() {
        let (_d, _store, ingest) = test_ingest();
        let _ = router(ingest.clone());
        let _ = router_with_token(ingest, Some("kro_secret".into()));
    }

    #[tokio::test]
    async fn trace_export_persists_and_requires_the_token() {
        let (_d, store, ingest) = test_ingest();
        let svc = OtlpGrpc {
            ingest,
            ingest_token: token("kro_secret"),
        };
        // Unauthenticated calls are refused before anything is written.
        let err = TraceService::export(&svc, Request::new(ExportTraceServiceRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);

        let mut req = Request::new(ExportTraceServiceRequest::default());
        req.metadata_mut().insert(
            "authorization",
            MetadataValue::from_static("Bearer kro_secret"),
        );
        let resp = TraceService::export(&svc, req).await.unwrap();
        assert!(resp.get_ref().partial_success.is_none());
        // The (empty) batch rode the writer channel without error; the store
        // is readable afterwards.
        assert!(store
            .read_only()
            .unwrap()
            .query_json_rows("SELECT 1 AS v")
            .is_ok());
    }

    #[tokio::test]
    async fn metrics_and_logs_export_persist_rows() {
        use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
        use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
        use opentelemetry_proto::tonic::metrics::v1::{
            metric::Data, Metric, ResourceMetrics, ScopeMetrics, Sum,
        };
        use opentelemetry_proto::tonic::resource::v1::Resource;

        let (_d, store, ingest) = test_ingest();
        let svc = OtlpGrpc {
            ingest,
            ingest_token: None,
        };

        let metrics_req = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                resource: Some(Resource {
                    attributes: vec![KeyValue {
                        key: "service.name".into(),
                        value: Some(AnyValue {
                            value: Some(
                                opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                                    "tumult".into(),
                                ),
                            ),
                        }),
                        key_strindex: 0,
                    }],
                    dropped_attributes_count: 0,
                    entity_refs: vec![],
                }),
                scope_metrics: vec![ScopeMetrics {
                    scope: None,
                    metrics: vec![Metric {
                        name: "tumult.experiments.total".into(),
                        description: String::new(),
                        unit: String::new(),
                        data: Some(Data::Sum(Sum {
                            data_points: vec![],
                            aggregation_temporality: 2,
                            is_monotonic: true,
                        })),
                        metadata: vec![],
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        MetricsService::export(&svc, Request::new(metrics_req))
            .await
            .unwrap();

        let logs_req = ExportLogsServiceRequest {
            resource_logs: vec![ResourceLogs {
                resource: None,
                scope_logs: vec![ScopeLogs {
                    scope: None,
                    log_records: vec![LogRecord {
                        time_unix_nano: 1_785_268_000_000_000_000,
                        severity_text: "INFO".into(),
                        body: Some(AnyValue {
                            value: Some(
                                opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                                    "experiment.started".into(),
                                ),
                            ),
                        }),
                        ..LogRecord::default()
                    }],
                    schema_url: String::new(),
                }],
                schema_url: String::new(),
            }],
        };
        LogsService::export(&svc, Request::new(logs_req))
            .await
            .unwrap();

        let rows = store
            .read_only()
            .unwrap()
            .query_json_rows("SELECT body FROM logs")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["body"], serde_json::json!("experiment.started"));
    }

    #[tokio::test]
    async fn logs_export_stores_entity_events_only_with_the_flag() {
        use crate::entity_events::tests as fixtures;

        for (legacy, wire_form) in [(false, "spec"), (true, "legacy")] {
            let (_d, store, ingest) = fixtures::ingest_with(fixtures::enabled());
            let svc = OtlpGrpc {
                ingest,
                ingest_token: None,
            };
            LogsService::export(&svc, Request::new(fixtures::host_export(legacy)))
                .await
                .unwrap();
            fixtures::assert_host_entity_and_one_log(&store, wire_form);
        }

        let (_d, store, ingest) = fixtures::ingest_with(None);
        let svc = OtlpGrpc {
            ingest,
            ingest_token: None,
        };
        LogsService::export(&svc, Request::new(fixtures::host_export(false)))
            .await
            .unwrap();
        fixtures::assert_two_logs_and_no_entity(&store);
    }

    /// What the export limit leaves out is in the answer over gRPC as it
    /// is over HTTP: the count and the reason.
    #[tokio::test]
    async fn events_beyond_the_export_limit_are_reported_to_the_sender() {
        use crate::entity_events::tests as fixtures;

        let (_d, store, ingest) = fixtures::ingest_with(fixtures::enabled());
        let svc = OtlpGrpc {
            ingest,
            ingest_token: None,
        };
        let clean = LogsService::export(&svc, Request::new(fixtures::host_export(false)))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(clean.partial_success, None);

        let mut export = fixtures::host_export(false);
        let state = export.resource_logs[0].scope_logs[0].log_records[0].clone();
        export.resource_logs[0].scope_logs[0].log_records =
            vec![state; tumult_otlp::MAX_EVENTS_PER_EXPORT + 3];
        let answer = LogsService::export(&svc, Request::new(export))
            .await
            .unwrap()
            .into_inner();
        let partial = answer.partial_success.expect("three events were dropped");
        assert_eq!(partial.rejected_log_records, 3);
        assert_eq!(
            partial.error_message,
            "entity events not stored: export_limit=3"
        );
        assert_eq!(fixtures::rows(&store, fixtures::ENTITIES).len(), 1);
    }

    #[tokio::test]
    async fn export_maps_writer_failure_to_internal() {
        // A dead writer task: the channel receiver is gone, so the export
        // call surfaces the channel error as INTERNAL (never a panic).
        let dead = OtlpGrpc {
            ingest: IngestWriter::stopped_for_test(),
            ingest_token: None,
        };
        let err = TraceService::export(&dead, Request::new(ExportTraceServiceRequest::default()))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::Internal);
    }
}

/// The router behind the daemon's listener, over in-memory connections and
/// a paused clock: what a peer that stalls, stops answering, never
/// authenticates or stays for ever sees on the wire, and when its place
/// under the limits comes back.
#[cfg(test)]
mod wire_tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::time::Instant;
    use tonic::metadata::MetadataValue;
    use tonic::transport::{Certificate, Channel, ClientTlsConfig, Endpoint};

    use opentelemetry_proto::tonic::collector::trace::v1::trace_service_client::TraceServiceClient;

    use super::*;
    use crate::listener::testing::{
        dialler, from, hello_offering_early_data, server_tls, undecryptable_record, Dial, CERT_PEM,
    };
    use crate::listener::{Access, ConnectionLimits, Gate, Incoming};
    use crate::otap::OtapLimits;

    const PEER: &str = "192.0.2.1";
    const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
    /// An empty SETTINGS frame, and the acknowledgement of the server's.
    const SETTINGS: &[u8] = &[0, 0, 0, 4, 0, 0, 0, 0, 0];
    const SETTINGS_ACK: &[u8] = &[0, 0, 0, 4, 1, 0, 0, 0, 0];
    const FRAME_SETTINGS: u8 = 4;
    const FRAME_PING: u8 = 6;
    const FLAG_ACK: u8 = 1;
    const TOKEN: &str = "kro_secret";

    struct Wired {
        gate: Gate,
        dial: Dial,
        _dir: tempfile::TempDir,
        _store: tumult_lake::Store,
    }

    /// Serve the daemon's router behind the daemon's listener, on
    /// connections made by [`Wired::dial`].
    fn wired(limits: ConnectionLimits, tls: bool) -> Wired {
        wired_with(limits, tls, None)
    }

    /// [`wired`], with the ingest token every call must carry.
    fn wired_with(limits: ConnectionLimits, tls: bool, token: Option<&str>) -> Wired {
        let dir = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&dir.path().join("k.duckdb")).unwrap();
        let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 16);
        let access = match token {
            Some(_) => Access::Token,
            None => Access::Open,
        };
        let token = token.map(str::to_owned);
        let router = build(ingest, token, OtapLimits::default(), limits);
        let (dial, dialled) = dialler();
        let mut incoming = Incoming::over(dialled, limits, access);
        if tls {
            incoming = incoming.with_tls(server_tls());
        }
        let gate = incoming.gate();
        tokio::spawn(router.serve_with_incoming(incoming));
        Wired {
            gate,
            dial,
            _dir: dir,
            _store: store,
        }
    }

    /// Limits under which the time to authenticate differs from every
    /// other time limit and from the first keepalive ping (30 s).
    fn auth_in_25s() -> ConnectionLimits {
        ConnectionLimits {
            auth_timeout: secs(25),
            ..ConnectionLimits::default()
        }
    }

    impl Wired {
        /// A new connection, once the listener has taken it in; the
        /// client's end.
        async fn dial(&self) -> DuplexStream {
            let before = self.gate.open();
            let client = self.dial.connect(from(PEER));
            self.until("the listener did not take the connection", || {
                self.gate.open() > before
            })
            .await;
            client
        }

        /// Wait, in paused time, for `done`.
        async fn until(&self, otherwise: &str, done: impl Fn() -> bool) {
            let waited = tokio::time::timeout(secs(60), async {
                while !done() {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            });
            waited.await.expect(otherwise);
        }

        /// Wait, in paused time, for the server to let go of every
        /// connection.
        async fn all_slots_back(&self) {
            self.until("the server still holds a connection", || {
                self.gate.open() == 0
            })
            .await;
        }

        /// A gRPC channel over a new connection.
        async fn channel(&self, tls: bool) -> Channel {
            channel_over(self.dial().await, tls).await
        }
    }

    /// A gRPC channel over `io`, and over no other connection.
    async fn channel_over(io: DuplexStream, tls: bool) -> Channel {
        let mut io = Some(io);
        let connector = tower::service_fn(move |_: tonic::transport::Uri| {
            let io = io.take().map(hyper_util::rt::TokioIo::new);
            async move { io.ok_or_else(|| std::io::Error::other("one connection only")) }
        });
        let endpoint = if tls {
            let tls = ClientTlsConfig::new()
                .ca_certificate(Certificate::from_pem(CERT_PEM))
                .domain_name("localhost");
            Endpoint::from_static("https://localhost")
                .tls_config(tls)
                .unwrap()
        } else {
            Endpoint::from_static("http://localhost")
        };
        endpoint
            .connect_with_connector(connector)
            .await
            .expect("a channel over the listener's connection")
    }

    /// `message` as a call that carries `bearer`.
    fn with_bearer<T>(message: T, bearer: &'static str) -> Request<T> {
        let mut request = Request::new(message);
        request
            .metadata_mut()
            .insert("authorization", MetadataValue::from_static(bearer));
        request
    }

    /// An export that carries the token.
    fn export() -> Request<ExportTraceServiceRequest> {
        with_bearer(ExportTraceServiceRequest::default(), "Bearer kro_secret")
    }

    /// Read frames until the server closes the connection, acknowledging
    /// its SETTINGS and, when `answer_pings`, its keepalive pings. Yields
    /// the types of the frames that arrived.
    async fn frames_until_closed(mut client: DuplexStream, answer_pings: bool) -> Vec<u8> {
        let mut seen = Vec::new();
        loop {
            let mut header = [0u8; 9];
            if client.read_exact(&mut header).await.is_err() {
                return seen;
            }
            let len =
                usize::from(header[0]) << 16 | usize::from(header[1]) << 8 | usize::from(header[2]);
            let mut payload = vec![0u8; len];
            if client.read_exact(&mut payload).await.is_err() {
                return seen;
            }
            let (kind, ack) = (header[3], header[4] & FLAG_ACK != 0);
            seen.push(kind);
            let reply = match kind {
                FRAME_SETTINGS if !ack => SETTINGS_ACK.to_vec(),
                FRAME_PING if !ack && answer_pings => {
                    let mut pong = vec![0, 0, 8, FRAME_PING, FLAG_ACK, 0, 0, 0, 0];
                    pong.extend_from_slice(&payload);
                    pong
                }
                _ => continue,
            };
            if client.write_all(&reply).await.is_err() {
                return seen;
            }
        }
    }

    /// An HTTP/2 client that completes the preface, answers every ping
    /// and never makes a call; yields the frame types it saw when the
    /// server closes the connection.
    async fn idle_until_closed(mut client: DuplexStream) -> Vec<u8> {
        client.write_all(PREFACE).await.unwrap();
        client.write_all(SETTINGS).await.unwrap();
        frames_until_closed(client, true).await
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn assert_took(started: Instant, expected: Duration) {
        let took = started.elapsed();
        assert!(
            took >= expected && took < expected + secs(2),
            "took {took:?}, expected {expected:?}"
        );
    }

    /// A day of paused time: a connection the server never closes fails
    /// the test instead of hanging it.
    async fn closed<T>(ended: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(secs(24 * 60 * 60), ended)
            .await
            .expect("the server never closed the connection")
    }

    /// Everything the server sends until it closes the connection.
    async fn bytes_until_closed(client: &mut DuplexStream) -> Vec<u8> {
        let mut seen = Vec::new();
        // The read fails or ends, depending on how the server lets go.
        let _ = closed(client.read_to_end(&mut seen)).await;
        seen
    }

    #[tokio::test(start_paused = true)]
    async fn the_server_drops_a_silent_connection_at_the_handshake_deadline() {
        let wired = wired(ConnectionLimits::default(), false);
        let started = Instant::now();
        let mut silent = wired.dial().await;
        assert_eq!(wired.gate.open(), 1);
        // The server speaks first (its SETTINGS); the client says nothing.
        bytes_until_closed(&mut silent).await;
        assert_took(started, secs(10));
        wired.all_slots_back().await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_server_drops_a_stalled_tls_handshake_at_the_deadline() {
        let wired = wired(ConnectionLimits::default(), true);
        let started = Instant::now();
        let mut client = wired.dial().await;
        // The header of a 512-byte handshake record and a few of its bytes.
        client
            .write_all(&[0x16, 3, 1, 2, 0, 1, 0, 1, 252])
            .await
            .unwrap();
        bytes_until_closed(&mut client).await;
        assert_took(started, secs(10));
        wired.all_slots_back().await;
    }

    /// A hello that offers early data, then records that decrypt to
    /// nothing: the server answers the hello, skips the records as TLS 1.3
    /// tells it to, and the handshake never completes. Bytes going by are
    /// not a handshake.
    #[tokio::test(start_paused = true)]
    async fn early_data_that_never_becomes_a_handshake_is_cut_at_the_deadline() {
        let wired = wired(ConnectionLimits::default(), true);
        let started = Instant::now();
        let mut client = wired.dial().await;
        client
            .write_all(&hello_offering_early_data())
            .await
            .unwrap();
        for _ in 0..3 {
            client.write_all(&undecryptable_record(64)).await.unwrap();
        }
        let seen = bytes_until_closed(&mut client).await;
        assert!(seen.starts_with(&[0x16, 3, 3]), "no ServerHello: {seen:?}");
        assert_took(started, secs(10));
        wired.all_slots_back().await;
    }

    /// The HTTP/2 keepalive: a ping after 30 s without traffic, and the
    /// connection is dropped when it is not answered within 20 s.
    #[tokio::test(start_paused = true)]
    async fn a_peer_that_stops_answering_pings_loses_its_connection() {
        let wired = wired(ConnectionLimits::default(), false);
        let started = Instant::now();
        let mut client = wired.dial().await;
        client.write_all(PREFACE).await.unwrap();
        client.write_all(SETTINGS).await.unwrap();
        let seen = closed(frames_until_closed(client, false)).await;
        assert!(seen.contains(&FRAME_PING), "no keepalive ping in {seen:?}");
        assert_took(started, HTTP2_KEEPALIVE_INTERVAL + HTTP2_KEEPALIVE_TIMEOUT);
        wired.all_slots_back().await;
    }

    /// Without a token a client that answers every ping keeps its
    /// connection until age and grace are over, and no longer: answering
    /// pings costs a peer nothing, so it must not buy a place for ever.
    #[tokio::test(start_paused = true)]
    async fn a_live_connection_ends_when_its_age_and_grace_are_over() {
        let limits = ConnectionLimits {
            max_connection_age: secs(400),
            max_connection_age_grace: secs(60),
            ..ConnectionLimits::default()
        };
        let wired = wired(limits, false);
        let started = Instant::now();
        let frames = tokio::spawn(idle_until_closed(wired.dial().await));
        tokio::time::sleep(secs(459)).await;
        assert!(
            !frames.is_finished(),
            "closed before age and grace were over"
        );
        assert_eq!(wired.gate.open(), 1);

        let seen = closed(frames).await.unwrap();
        assert!(seen.contains(&FRAME_PING), "no keepalive ping in {seen:?}");
        assert_took(started, secs(460));
        wired.all_slots_back().await;
    }

    /// With a token configured, a client that completes HTTP/2 and
    /// answers every ping but never makes an authorised call does not
    /// keep its place.
    #[tokio::test(start_paused = true)]
    async fn a_connection_that_never_authenticates_is_cut_at_the_deadline() {
        let wired = wired_with(auth_in_25s(), false, Some(TOKEN));
        let started = Instant::now();
        let client = wired.dial().await;
        assert_eq!((wired.gate.open(), wired.gate.unauthenticated()), (1, 1));
        closed(idle_until_closed(client)).await;
        assert_took(started, secs(25));
        wired.all_slots_back().await;
    }

    /// A call with the wrong token, or with none, is a call all the same
    /// but not an authorised one: the connection goes when its time is up.
    #[tokio::test(start_paused = true)]
    async fn calls_without_the_token_do_not_keep_a_connection() {
        let wired = wired_with(auth_in_25s(), false, Some(TOKEN));
        let started = Instant::now();
        let mut client = TraceServiceClient::new(wired.channel(false).await);
        for bearer in ["Bearer kro_other", "kro_secret"] {
            let request = with_bearer(ExportTraceServiceRequest::default(), bearer);
            let refused = client.export(request).await.unwrap_err();
            assert_eq!(refused.code(), tonic::Code::Unauthenticated, "{refused}");
        }
        let refused = client.export(ExportTraceServiceRequest::default()).await;
        assert_eq!(refused.unwrap_err().code(), tonic::Code::Unauthenticated);
        assert_eq!((wired.gate.open(), wired.gate.unauthenticated()), (1, 1));

        wired.all_slots_back().await;
        assert_took(started, secs(25));
    }

    /// One authorised unary call and the connection is a client's: it
    /// stays past the time to authenticate.
    #[tokio::test(start_paused = true)]
    async fn an_authorised_export_keeps_its_connection() {
        let wired = wired_with(auth_in_25s(), false, Some(TOKEN));
        let mut client = TraceServiceClient::new(wired.channel(false).await);
        tokio::time::sleep(secs(20)).await;
        assert_eq!((wired.gate.open(), wired.gate.unauthenticated()), (1, 1));
        client.export(export()).await.expect("first export");
        assert_eq!((wired.gate.open(), wired.gate.unauthenticated()), (1, 0));

        tokio::time::sleep(secs(300)).await;
        assert_eq!(wired.gate.open(), 1, "an authenticated connection was cut");
        // Still the same connection: the connector has no second one.
        client.export(export()).await.expect("a later export");
    }

    /// An OTel-Arrow stream is authorised when it opens, and that counts
    /// like a unary call.
    #[tokio::test(start_paused = true)]
    async fn an_authorised_arrow_stream_keeps_its_connection() {
        use crate::otap::proto::arrow_traces_service_client::ArrowTracesServiceClient;
        use crate::otap::proto::BatchArrowRecords;
        use tonic::codegen::tokio_stream::wrappers::ReceiverStream;

        let wired = wired_with(auth_in_25s(), false, Some(TOKEN));
        let mut client = ArrowTracesServiceClient::new(wired.channel(false).await);
        let open = |bearer| {
            let (tx, rx) = tokio::sync::mpsc::channel::<BatchArrowRecords>(1);
            (tx, with_bearer(ReceiverStream::new(rx), bearer))
        };
        let (_tx, request) = open("Bearer kro_other");
        let refused = client.arrow_traces(request).await.unwrap_err();
        assert_eq!(refused.code(), tonic::Code::Unauthenticated, "{refused}");
        assert_eq!((wired.gate.open(), wired.gate.unauthenticated()), (1, 1));

        let (_tx, request) = open("Bearer kro_secret");
        let _acks = client.arrow_traces(request).await.expect("stream opened");
        assert_eq!((wired.gate.open(), wired.gate.unauthenticated()), (1, 0));
        tokio::time::sleep(secs(300)).await;
        assert_eq!(wired.gate.open(), 1, "an authenticated connection was cut");
    }

    /// Sixteen peers with sixty-four idle connections each used to hold
    /// every place. Now the oldest connection that never authenticated
    /// goes the moment a newcomer needs its place, and the newcomer's
    /// call is served.
    #[tokio::test(start_paused = true)]
    async fn a_full_listener_serves_a_newcomer_in_place_of_an_idle_connection() {
        let limits = ConnectionLimits {
            max_connections: 2,
            max_per_peer: 2,
            ..auth_in_25s()
        };
        let wired = wired_with(limits, false, Some(TOKEN));
        let mut established = TraceServiceClient::new(wired.channel(false).await);
        established.export(export()).await.expect("first client");
        let idle = tokio::spawn(idle_until_closed(wired.dial().await));
        assert_eq!((wired.gate.open(), wired.gate.unauthenticated()), (2, 1));

        tokio::time::sleep(secs(5)).await;
        let started = Instant::now();
        // The listener is full, so the count does not rise: connect, and
        // let the call show that the newcomer got in.
        let io = wired.dial.connect(from(PEER));
        let mut newcomer = TraceServiceClient::new(channel_over(io, false).await);
        newcomer.export(export()).await.expect("the newcomer");
        closed(idle).await.unwrap();
        assert_took(started, secs(0));
        assert_eq!((wired.gate.open(), wired.gate.unauthenticated()), (2, 0));

        // Nobody is left to put out: the listener is full of clients.
        let mut refused = wired.dial.connect(from(PEER));
        assert!(bytes_until_closed(&mut refused).await.is_empty());
        established.export(export()).await.expect("first client");
        newcomer.export(export()).await.expect("the newcomer");
    }

    /// A real TLS client: the listener completes its handshake and sees
    /// its preface, so it is still served long after the deadline.
    #[tokio::test(start_paused = true)]
    async fn a_tls_client_is_served_past_the_handshake_deadline() {
        let wired = wired_with(ConnectionLimits::default(), true, Some(TOKEN));
        let mut client = TraceServiceClient::new(wired.channel(true).await);
        client.export(export()).await.expect("first export");
        assert_eq!((wired.gate.open(), wired.gate.unauthenticated()), (1, 0));

        tokio::time::sleep(secs(300)).await;
        assert_eq!(wired.gate.open(), 1, "the TLS connection was cut");
        // Still the same connection: the connector has no second one.
        client
            .export(export())
            .await
            .expect("export after the deadline");
    }
}
