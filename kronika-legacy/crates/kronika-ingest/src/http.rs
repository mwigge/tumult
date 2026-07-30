//! OTLP/HTTP endpoints (what smedja's exporter talks).
//!
//! `POST /v1/traces|/v1/metrics|/v1/logs` accept `application/x-protobuf`
//! bodies carrying the OTLP export requests, decode them with prost, and
//! funnel the resulting rows into the single-writer channel.
//! `GET /healthz` is the daemon health endpoint.

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;

use crate::error::IngestError;
use crate::writer::{Batch, IngestWriter};

/// Build the HTTP router (OTLP/HTTP + health).
pub fn router(ingest: IngestWriter) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/traces", post(traces))
        .route("/v1/metrics", post(metrics))
        .route("/v1/logs", post(logs))
        .with_state(ingest)
}

async fn healthz() -> &'static str {
    "ok"
}

async fn traces(State(ingest): State<IngestWriter>, body: Bytes) -> impl IntoResponse {
    match decode::<ExportTraceServiceRequest>(&body).await {
        Ok(request) => {
            let spans = kronika_otel::trace_request_to_spans(&request);
            let count = spans.len();
            match ingest.write(Batch::Spans(spans)).await {
                Ok(()) => (StatusCode::OK, format!("{count} spans ingested")).into_response(),
                Err(e) => server_error(e),
            }
        }
        Err(e) => client_error(e),
    }
}

async fn metrics(State(ingest): State<IngestWriter>, body: Bytes) -> impl IntoResponse {
    match decode::<ExportMetricsServiceRequest>(&body).await {
        Ok(request) => {
            let rows = kronika_otel::metrics_request_to_rows(&request);
            let count = rows.sums.len() + rows.gauges.len() + rows.histograms.len();
            match ingest.write(Batch::Metrics(rows)).await {
                Ok(()) => (StatusCode::OK, format!("{count} data points ingested")).into_response(),
                Err(e) => server_error(e),
            }
        }
        Err(e) => client_error(e),
    }
}

async fn logs(State(ingest): State<IngestWriter>, body: Bytes) -> impl IntoResponse {
    match decode::<ExportLogsServiceRequest>(&body).await {
        Ok(request) => {
            let rows = kronika_otel::logs_request_to_rows(&request, crate::now_ns());
            let count = rows.len();
            match ingest.write(Batch::Logs(rows)).await {
                Ok(()) => (StatusCode::OK, format!("{count} log records ingested")).into_response(),
                Err(e) => server_error(e),
            }
        }
        Err(e) => client_error(e),
    }
}

async fn decode<T: Message + Default>(body: &[u8]) -> Result<T, IngestError> {
    Ok(T::decode(body)?)
}

fn client_error(e: IngestError) -> axum::response::Response {
    (StatusCode::BAD_REQUEST, e.to_string()).into_response()
}

fn server_error(e: IngestError) -> axum::response::Response {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response()
}
