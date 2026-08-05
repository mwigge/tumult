//! OTLP/HTTP endpoints (what smedja's exporter talks).
//!
//! `POST /v1/traces|/v1/metrics|/v1/logs` accept `application/x-protobuf`
//! bodies carrying the OTLP export requests, decode them with prost, and
//! funnel the resulting rows into the single-writer channel.
//!
//! When an ingest token is configured (`KRONIKA_INGEST_TOKEN`), every
//! `/v1/*` route requires `Authorization: Bearer <token>`; non-`/v1` routes
//! pass through. The daemon's `/healthz`, `/readyz` and `/metrics` live on
//! tumultd's ops router (behind the API auth middleware), not here.

use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;

use crate::error::IngestError;
use crate::writer::{Batch, IngestWriter};

/// Build the HTTP router (OTLP/HTTP), unauthenticated.
pub fn router(ingest: IngestWriter) -> Router {
    router_with_token(ingest, None)
}

/// Build the HTTP router; when `ingest_token` is `Some`, every `/v1/*`
/// route requires `Authorization: Bearer <token>` (constant-time compare).
pub fn router_with_token(ingest: IngestWriter, ingest_token: Option<String>) -> Router {
    let router = Router::new()
        .route("/v1/traces", post(traces))
        .route("/v1/metrics", post(metrics))
        .route("/v1/logs", post(logs));
    let router = match ingest_token {
        Some(token) => router.layer(middleware::from_fn_with_state(
            std::sync::Arc::new(format!("Bearer {token}")),
            require_bearer,
        )),
        None => router,
    };
    router.with_state(ingest)
}

/// Bearer-token guard for the `/v1/*` OTLP routes; every other path passes
/// through. The fail-closed startup guard that refuses a token-less
/// non-loopback bind lives on [`crate::Config::ensure_ingest_auth`].
async fn require_bearer(
    State(expected): State<std::sync::Arc<String>>,
    req: Request,
    next: Next,
) -> Response {
    if req.uri().path().starts_with("/v1/") {
        let presented = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        let authorized = presented.is_some_and(|v| tumult_auth::constant_time_eq(v, &expected));
        if !authorized {
            return (
                StatusCode::UNAUTHORIZED,
                axum::Json(serde_json::json!({"error": "unauthorized"})),
            )
                .into_response();
        }
    }
    next.run(req).await
}

async fn traces(State(ingest): State<IngestWriter>, body: Bytes) -> impl IntoResponse {
    match decode::<ExportTraceServiceRequest>(&body).await {
        Ok(request) => {
            let spans = tumult_otlp::trace_request_to_spans(&request);
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
            let rows = tumult_otlp::metrics_request_to_rows(&request);
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
            let rows = tumult_otlp::logs_request_to_rows(&request, crate::now_ns());
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
