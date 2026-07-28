//! OTLP/gRPC services (what tumult's exporter talks: bare `host:4317`).

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
use tonic::{Request, Response, Status};

use crate::writer::{Batch, IngestWriter};

/// Build the tonic gRPC router for the three OTLP collector services.
pub fn router(ingest: IngestWriter) -> Router {
    tonic::transport::Server::builder()
        .add_service(TraceServiceServer::new(OtlpGrpc {
            ingest: ingest.clone(),
        }))
        .add_service(MetricsServiceServer::new(OtlpGrpc {
            ingest: ingest.clone(),
        }))
        .add_service(LogsServiceServer::new(OtlpGrpc { ingest }))
}

#[derive(Clone)]
struct OtlpGrpc {
    ingest: IngestWriter,
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
        let spans = kronika_otel::trace_request_to_spans(request.get_ref());
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
        let rows = kronika_otel::metrics_request_to_rows(request.get_ref());
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
        let rows = kronika_otel::logs_request_to_rows(request.get_ref());
        self.ingest
            .write(Batch::Logs(rows))
            .await
            .map_err(to_status)?;
        Ok(Response::new(ExportLogsServiceResponse {
            partial_success: None,
        }))
    }
}
