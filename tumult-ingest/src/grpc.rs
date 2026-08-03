//! OTLP/gRPC services (what tumult's exporter talks: bare `host:4317`).
//!
//! When an ingest token is configured (`KRONIKA_INGEST_TOKEN`), every
//! export call must carry an `authorization: Bearer <token>` metadata entry;
//! anything else is rejected with `UNAUTHENTICATED`.

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
use tonic::transport::server::Router;
use tonic::transport::{Identity, ServerTlsConfig};
use tonic::{Request, Response, Status};

use crate::writer::{Batch, IngestWriter};

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
    let token = ingest_token.map(std::sync::Arc::new);
    let builder = tonic::transport::Server::builder();
    let mut builder = match identity {
        Some(identity) => builder.tls_config(ServerTlsConfig::new().identity(identity))?,
        None => builder,
    };
    Ok(builder
        .add_service(TraceServiceServer::new(OtlpGrpc {
            ingest: ingest.clone(),
            ingest_token: token.clone(),
        }))
        .add_service(MetricsServiceServer::new(OtlpGrpc {
            ingest: ingest.clone(),
            ingest_token: token.clone(),
        }))
        .add_service(LogsServiceServer::new(OtlpGrpc {
            ingest,
            ingest_token: token,
        })))
}

/// Bearer-token check for one incoming call: `None` token accepts
/// everything; otherwise the `authorization` metadata must be exactly
/// `Bearer <token>` (constant-time compare).
fn authorize<T>(
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
        _ => Err(Status::unauthenticated("missing or invalid bearer token")),
    }
}

#[derive(Clone)]
struct OtlpGrpc {
    ingest: IngestWriter,
    ingest_token: Option<std::sync::Arc<String>>,
}

fn to_status(e: crate::error::IngestError) -> Status {
    Status::internal(e.to_string())
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
        self.ingest
            .write(Batch::Metrics(rows))
            .await
            .map_err(to_status)?;
        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
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

    fn test_ingest() -> (tempfile::TempDir, tumult_lake::Store, IngestWriter) {
        let d = tempfile::TempDir::new().unwrap();
        let store = tumult_lake::Store::open(&d.path().join("k.duckdb")).unwrap();
        let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 16);
        (d, store, ingest)
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
