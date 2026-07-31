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
}
