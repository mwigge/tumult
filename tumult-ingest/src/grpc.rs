//! OTLP/gRPC services (what tumult's exporter talks: bare `host:4317`),
//! plus the OTel-Arrow stream services ([`crate::otap`]) on the same
//! listener.
//!
//! When an ingest token is configured (`KRONIKA_INGEST_TOKEN`), every
//! export call (and every Arrow stream) must carry an
//! `authorization: Bearer <token>` metadata entry; anything else is rejected
//! with `UNAUTHENTICATED`. The token is checked on the headers of a call
//! ([`bearer`]), before its message is read or decoded.

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
use tonic::transport::server::Router;
use tonic::transport::{Identity, ServerTlsConfig};

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
    router_with_token_tls(ingest, ingest_token, None)
        .expect("router without a TLS identity cannot fail")
}

/// Build the tonic gRPC router with an optional TLS identity (PEM certificate
/// chain + private key from `KRONIKA_TLS_CERT` / `KRONIKA_TLS_KEY`). An
/// invalid identity fails here, at startup, before the server binds.
pub fn router_with_token_tls(
    ingest: IngestWriter,
    ingest_token: Option<String>,
    identity: Option<Identity>,
) -> Result<Router, tonic::transport::Error> {
    // Arrow stream limits (rows, attributes, decoded bytes, message size,
    // open streams, idle time) and the connection time limits come from
    // the environment, see `OtapLimits` and `ConnectionLimits`.
    build(
        ingest,
        ingest_token,
        identity,
        crate::otap::OtapLimits::from_env(),
        crate::listener::ConnectionLimits::from_env(),
    )
}

/// [`router_with_token_tls`] with the limits given.
fn build(
    ingest: IngestWriter,
    ingest_token: Option<String>,
    identity: Option<Identity>,
    otap_limits: crate::otap::OtapLimits,
    connections: crate::listener::ConnectionLimits,
) -> Result<Router, tonic::transport::Error> {
    let token = ingest_token.map(std::sync::Arc::new);
    // A connection is dropped when its age and grace are over; the
    // listener (`listener::LimitedStream`) enforces the same limit on the
    // socket itself. Both settings must be given to tonic 0.14: with the
    // age alone its connection task polls a finished timer a second time
    // and panics, which aborts a release build.
    let builder = tonic::transport::Server::builder()
        .max_concurrent_streams(MAX_CONCURRENT_STREAMS_PER_CONNECTION)
        .concurrency_limit_per_connection(MAX_CONCURRENT_REQUESTS_PER_CONNECTION)
        .http2_keepalive_interval(Some(HTTP2_KEEPALIVE_INTERVAL))
        .http2_keepalive_timeout(Some(HTTP2_KEEPALIVE_TIMEOUT))
        .max_connection_age(connections.max_connection_age)
        .max_connection_age_grace(connections.max_connection_age_grace);
    let mut builder = match identity {
        Some(identity) => builder.tls_config(ServerTlsConfig::new().identity(identity))?,
        None => builder,
    };
    let otap =
        crate::otap::service::OtapGrpc::with_limits(ingest.clone(), token.clone(), otap_limits);
    let otlp = OtlpGrpc {
        ingest,
        ingest_token: token.clone(),
    };
    Ok(builder
        .add_service(TraceServiceServer::with_interceptor(
            otlp.clone(),
            bearer(token.clone()),
        ))
        .add_service(MetricsServiceServer::with_interceptor(
            otlp.clone(),
            bearer(token.clone()),
        ))
        .add_service(LogsServiceServer::with_interceptor(otlp, bearer(token)))
        // OTel-Arrow: same listener, same token, same writer channel.
        .add_service(otap.clone().traces())
        .add_service(otap.clone().logs())
        .add_service(otap.metrics()))
}

/// Bearer-token check for one incoming call: `None` token accepts
/// everything; otherwise the `authorization` metadata must be exactly
/// `Bearer <token>` (constant-time compare). A rejection is logged at WARN
/// (rate-limited, with a count of what was left out), saying only whether
/// a credential was presented, never what it was.
pub(crate) fn authorize<T>(
    ingest_token: Option<&std::sync::Arc<String>>,
    request: &Request<T>,
) -> Result<(), Status> {
    let Some(token) = ingest_token else {
        return Ok(());
    };
    let expected = format!("Bearer {token}");
    let presented = request
        .metadata()
        .get("authorization")
        .and_then(|v| v.to_str().ok());
    match presented {
        Some(v) if tumult_auth::constant_time_eq(v, &expected) => Ok(()),
        _ => {
            let peer = request.remote_addr().map(|peer| peer.ip());
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
        let rows = tumult_otlp::logs_request_to_rows(request.get_ref(), crate::now_ns());
        self.ingest
            .write(Batch::Logs(rows))
            .await
            .map_err(to_status)?;
        Ok(Response::new(ExportLogsServiceResponse {
            partial_success: None,
        }))
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
        // Plain, token-gated and explicit no-TLS builders all succeed. (TLS
        // identity handling needs the process-level rustls crypto provider,
        // which only the daemon installs at startup.)
        let _ = router(ingest.clone());
        let _ = router_with_token(ingest.clone(), Some("kro_secret".into()));
        assert!(router_with_token_tls(ingest, None, None).is_ok());
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

/// The router behind the listener's gate, over in-memory connections and a
/// paused clock: what a peer that stalls, stops answering or stays for ever
/// sees on the wire, and when its place under the limits comes back.
#[cfg(test)]
mod wire_tests {
    use std::net::{IpAddr, Ipv4Addr};

    use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};
    use tokio::sync::mpsc;
    use tokio::time::Instant;
    use tonic::codegen::tokio_stream::wrappers::ReceiverStream;

    use super::*;
    use crate::listener::{ConnectionLimits, Gate, LimitedStream};
    use crate::otap::OtapLimits;

    /// A self-signed certificate for `localhost`, valid until 2126, and its
    /// key. Test material only.
    const CERT_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBkzCCATigAwIBAgIUV5d+A5IchQED5lsdSaln9OgDvYUwCgYIKoZIzj0EAwIw
FDESMBAGA1UEAwwJbG9jYWxob3N0MCAXDTI2MTExMzIyMjg1MFoYDzIxMjYxMDIw
MjIyODUwWjAUMRIwEAYDVQQDDAlsb2NhbGhvc3QwWTATBgcqhkjOPQIBBggqhkjO
PQMBBwNCAATk8+CZpWzN4qurvjgDFwx1+mfl39zNcApbONrjg7vllmcms1THXVak
CO+C7keoTOT/4/iU7KRvdpggsjmr85/To2YwZDAdBgNVHQ4EFgQUunMxwUUBqvKV
MN184GRoL5b1vQ4wHwYDVR0jBBgwFoAUunMxwUUBqvKVMN184GRoL5b1vQ4wFAYD
VR0RBA0wC4IJbG9jYWxob3N0MAwGA1UdEwEB/wQCMAAwCgYIKoZIzj0EAwIDSQAw
RgIhAOS/bUiv0yGD4Pxb1PKS4EuuKigYlU63Et/c/cCDzN1BAiEAsjyu4cD/rOTe
Dg2Z0iprmD/PAnqqBe1X6rLeEubDzl0=
-----END CERTIFICATE-----
";
    const KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgrCcSpK4PJ/OyBCsN
+vDbaSdJ9nNtDAJ6Q2bjZfSeZHKhRANCAATk8+CZpWzN4qurvjgDFwx1+mfl39zN
cApbONrjg7vllmcms1THXVakCO+C7keoTOT/4/iU7KRvdpggsjmr85/T
-----END PRIVATE KEY-----
";

    const PEER: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1));
    const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
    /// An empty SETTINGS frame, and the acknowledgement of the server's.
    const SETTINGS: &[u8] = &[0, 0, 0, 4, 0, 0, 0, 0, 0];
    const SETTINGS_ACK: &[u8] = &[0, 0, 0, 4, 1, 0, 0, 0, 0];
    const FRAME_SETTINGS: u8 = 4;
    const FRAME_PING: u8 = 6;
    const FLAG_ACK: u8 = 1;

    struct Wired {
        gate: Gate,
        connections: mpsc::Sender<std::io::Result<LimitedStream<DuplexStream>>>,
        _dir: tempfile::TempDir,
        _store: tumult_lake::Store,
    }

    /// Serve the daemon's router on connections handed over by [`dial`].
    fn wired(limits: ConnectionLimits, tls: bool) -> Wired {
        let dir = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&dir.path().join("k.duckdb")).unwrap();
        let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 16);
        let identity = tls.then(|| {
            // Both rustls providers are compiled in; pick one, as the
            // daemon does at startup.
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
            Identity::from_pem(CERT_PEM, KEY_PEM)
        });
        let router = build(ingest, None, identity, OtapLimits::default(), limits).unwrap();
        let (connections, accepted) = mpsc::channel(16);
        tokio::spawn(router.serve_with_incoming(ReceiverStream::new(accepted)));
        Wired {
            gate: Gate::new(limits),
            connections,
            _dir: dir,
            _store: store,
        }
    }

    impl Wired {
        /// A new connection through the gate; the client's end.
        async fn dial(&self) -> DuplexStream {
            let (client, server) = tokio::io::duplex(1 << 16);
            let admitted = self.gate.admit(server, PEER).expect("under the limits");
            self.connections.send(Ok(admitted)).await.unwrap();
            client
        }

        /// Wait, in paused time, for the server to let go of every
        /// connection.
        async fn all_slots_back(&self) {
            let waited = tokio::time::timeout(Duration::from_secs(5), async {
                while self.gate.open() > 0 {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            });
            waited.await.expect("the server still holds a connection");
        }
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
    async fn closed(frames: impl std::future::Future<Output = Vec<u8>>) -> Vec<u8> {
        tokio::time::timeout(secs(24 * 60 * 60), frames)
            .await
            .expect("the server never closed the connection")
    }

    #[tokio::test(start_paused = true)]
    async fn the_server_drops_a_silent_connection_at_the_handshake_deadline() {
        let wired = wired(ConnectionLimits::default(), false);
        let started = Instant::now();
        let client = wired.dial().await;
        assert_eq!(wired.gate.open(), 1);
        // The server speaks first (its SETTINGS); the client says nothing.
        let mut silent = client;
        let mut seen = Vec::new();
        let read = tokio::time::timeout(secs(24 * 60 * 60), silent.read_to_end(&mut seen));
        read.await.expect("never closed").unwrap();
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
        let mut seen = Vec::new();
        let read = tokio::time::timeout(secs(24 * 60 * 60), client.read_to_end(&mut seen));
        // The read fails or ends, depending on how the server lets go.
        let _ = read.await.expect("never closed");
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

    /// A client that answers every ping keeps its connection until age
    /// and grace are over, and no longer: answering pings costs a peer
    /// nothing, so it must not buy a place for ever.
    #[tokio::test(start_paused = true)]
    async fn a_live_connection_ends_when_its_age_and_grace_are_over() {
        let limits = ConnectionLimits {
            max_connection_age: secs(400),
            max_connection_age_grace: secs(60),
            ..ConnectionLimits::default()
        };
        let wired = wired(limits, false);
        let started = Instant::now();
        let mut client = wired.dial().await;
        client.write_all(PREFACE).await.unwrap();
        client.write_all(SETTINGS).await.unwrap();
        let frames = tokio::spawn(frames_until_closed(client, true));
        tokio::time::sleep(secs(459)).await;
        assert!(
            !frames.is_finished(),
            "closed before age and grace were over"
        );
        assert_eq!(wired.gate.open(), 1);

        let seen = closed(async { frames.await.unwrap() }).await;
        assert!(seen.contains(&FRAME_PING), "no keepalive ping in {seen:?}");
        assert_took(started, secs(460));
        wired.all_slots_back().await;
    }

    /// A real TLS client: its handshake and preface are recognised from
    /// the record stream, so it is still served long after the deadline.
    #[tokio::test(start_paused = true)]
    async fn a_tls_client_is_served_past_the_handshake_deadline() {
        use opentelemetry_proto::tonic::collector::trace::v1::trace_service_client::TraceServiceClient;
        use tonic::transport::{Certificate, ClientTlsConfig, Endpoint};

        let wired = wired(ConnectionLimits::default(), true);
        let mut io = Some(wired.dial().await);
        let connector = tower::service_fn(move |_: tonic::transport::Uri| {
            let io = io.take().map(hyper_util::rt::TokioIo::new);
            async move { io.ok_or_else(|| std::io::Error::other("one connection only")) }
        });
        let tls = ClientTlsConfig::new()
            .ca_certificate(Certificate::from_pem(CERT_PEM))
            .domain_name("localhost");
        let channel = Endpoint::from_static("https://localhost")
            .tls_config(tls)
            .unwrap()
            .connect_with_connector(connector)
            .await
            .expect("TLS handshake over the gated connection");
        let mut client = TraceServiceClient::new(channel);
        client
            .export(ExportTraceServiceRequest::default())
            .await
            .expect("first export");

        tokio::time::sleep(secs(300)).await;
        assert_eq!(wired.gate.open(), 1, "the TLS connection was cut");
        // Still the same connection: the connector has no second one.
        client
            .export(ExportTraceServiceRequest::default())
            .await
            .expect("export after the deadline");
    }
}
