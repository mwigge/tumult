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
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsPartialSuccess, ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
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
            return status_response(StatusCode::UNAUTHORIZED, "unauthorized".into());
        }
    }
    next.run(req).await
}

async fn traces(State(ingest): State<IngestWriter>, body: Bytes) -> impl IntoResponse {
    match decode::<ExportTraceServiceRequest>(&body).await {
        Ok(request) => {
            let spans = tumult_otlp::trace_request_to_spans(&request);
            match ingest.write(Batch::Spans(spans)).await {
                Ok(()) => protobuf(ExportTraceServiceResponse::default()),
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
            let response = metrics_response(rows.rejected_data_points);
            match ingest.write(Batch::Metrics(rows)).await {
                Ok(()) => protobuf(response),
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
            match ingest.write(Batch::Logs(rows)).await {
                Ok(()) => protobuf(ExportLogsServiceResponse::default()),
                Err(e) => server_error(e),
            }
        }
        Err(e) => client_error(e),
    }
}

fn protobuf(message: impl Message) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-protobuf")],
        message.encode_to_vec(),
    )
        .into_response()
}

pub(crate) fn metrics_response(rejected: i64) -> ExportMetricsServiceResponse {
    ExportMetricsServiceResponse {
        partial_success: (rejected > 0).then(|| ExportMetricsPartialSuccess {
            rejected_data_points: rejected,
            error_message: "exponential histograms and summaries are not supported".into(),
        }),
    }
}

async fn decode<T: Message + Default>(body: &[u8]) -> Result<T, IngestError> {
    Ok(T::decode(body)?)
}

// google.rpc.Status wire format used by OTLP error responses.
#[derive(prost::Message)]
struct OtlpStatus {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
}

fn status_response(status: StatusCode, message: String) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "application/x-protobuf")],
        OtlpStatus {
            code: match status {
                StatusCode::BAD_REQUEST => tonic::Code::InvalidArgument as i32,
                StatusCode::UNAUTHORIZED => tonic::Code::Unauthenticated as i32,
                StatusCode::FORBIDDEN => tonic::Code::PermissionDenied as i32,
                StatusCode::TOO_MANY_REQUESTS => tonic::Code::ResourceExhausted as i32,
                StatusCode::SERVICE_UNAVAILABLE => tonic::Code::Unavailable as i32,
                _ => tonic::Code::Internal as i32,
            },
            message,
        }
        .encode_to_vec(),
    )
        .into_response()
}

fn client_error(e: IngestError) -> Response {
    status_response(StatusCode::BAD_REQUEST, e.to_string())
}

fn server_error(e: IngestError) -> axum::response::Response {
    tracing::error!(error = %e, "OTLP write failed");
    status_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "telemetry write failed".into(),
    )
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceResponse;
    use opentelemetry_proto::tonic::metrics::v1::{
        metric, ExponentialHistogram, ExponentialHistogramDataPoint, Gauge, Metric,
        NumberDataPoint, ResourceMetrics, ScopeMetrics, Summary, SummaryDataPoint,
    };

    #[tokio::test]
    async fn error_status_codes_and_messages_are_safe() {
        for (status, code) in [
            (StatusCode::BAD_REQUEST, 3),
            (StatusCode::UNAUTHORIZED, 16),
            (StatusCode::INTERNAL_SERVER_ERROR, 13),
        ] {
            let response = status_response(status, "safe".into());
            let body = axum::body::to_bytes(response.into_body(), 4096)
                .await
                .unwrap();
            assert_eq!(OtlpStatus::decode(body).unwrap().code, code);
        }
        let response = server_error(IngestError::Channel(
            "database path /private/customer-secret".into(),
        ));
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let message = OtlpStatus::decode(body).unwrap().message;
        assert!(!message.contains("customer-secret"));
    }

    #[tokio::test]
    async fn protobuf_success_and_rejection_counts() {
        let dir = tempfile::tempdir().unwrap();
        let store = tumult_lake::Store::open(&dir.path().join("db")).unwrap();
        let (ingest, task) = IngestWriter::spawn(store.writer().unwrap(), 4);
        let empty = metrics(State(ingest.clone()), Bytes::new())
            .await
            .into_response();
        assert_eq!(empty.headers()["content-type"], "application/x-protobuf");
        let body = axum::body::to_bytes(empty.into_body(), 4096).await.unwrap();
        assert!(ExportMetricsServiceResponse::decode(body)
            .unwrap()
            .partial_success
            .is_none());
        let request = ExportMetricsServiceRequest {
            resource_metrics: vec![ResourceMetrics {
                scope_metrics: vec![ScopeMetrics {
                    metrics: vec![
                        Metric {
                            name: "supported".into(),
                            data: Some(metric::Data::Gauge(Gauge {
                                data_points: vec![NumberDataPoint {
                                    time_unix_nano: 1,
                                    value: Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(4.2)),
                                    ..Default::default()
                                }],
                            })),
                            ..Default::default()
                        },
                        Metric {
                            data: Some(metric::Data::ExponentialHistogram(ExponentialHistogram {
                                data_points: vec![ExponentialHistogramDataPoint::default()],
                                ..Default::default()
                            })),
                            ..Default::default()
                        },
                        Metric {
                            data: Some(metric::Data::Summary(Summary {
                                data_points: vec![
                                    SummaryDataPoint::default(),
                                    SummaryDataPoint::default(),
                                ],
                            })),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };
        let response = metrics(State(ingest.clone()), Bytes::from(request.encode_to_vec()))
            .await
            .into_response();
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let partial = ExportMetricsServiceResponse::decode(body)
            .unwrap()
            .partial_success
            .unwrap();
        assert_eq!(partial.rejected_data_points, 3);
        assert!(!partial.error_message.is_empty());
        assert_eq!(
            store
                .read_only()
                .unwrap()
                .query_json_rows("SELECT count(*) AS n FROM metric_gauges")
                .unwrap()[0]["n"],
            1
        );
        drop(ingest);
        task.await.unwrap();
    }
}
