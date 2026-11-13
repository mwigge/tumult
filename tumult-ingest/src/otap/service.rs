//! The three OTel-Arrow gRPC stream services.
//!
//! Each stream is handled sequentially: a `BatchArrowRecords` is decoded,
//! converted with `tumult-otlp` and written through the single-writer
//! channel before its `BatchStatus` goes back, so the writer's backpressure
//! reaches the exporter the same way it does for unary OTLP exports.
//! Decode and write failures are reported in the batch's status and never
//! close the stream; only a failed stream read does.

use std::sync::Arc;
use std::task::{Context, Poll};

use tonic::codegen::tokio_stream::wrappers::ReceiverStream;
use tonic::codegen::{http, CompressionEncoding, Service};
use tonic::server::NamedService;
use tonic::{Request, Response, Status, Streaming};

use super::decode::StreamConsumer;
use super::proto::arrow_logs_service_server::{ArrowLogsService, ArrowLogsServiceServer};
use super::proto::arrow_metrics_service_server::{ArrowMetricsService, ArrowMetricsServiceServer};
use super::proto::arrow_traces_service_server::{ArrowTracesService, ArrowTracesServiceServer};
use super::proto::{BatchArrowRecords, BatchStatus, StatusCode};
use super::{to_logs_request, to_metrics_request, to_trace_request, OtapBatch, OtapError};
use crate::grpc::authorize;
use crate::writer::{Batch, IngestWriter};

/// Shared state of the three services.
#[derive(Clone)]
pub struct OtapGrpc {
    ingest: IngestWriter,
    ingest_token: Option<Arc<String>>,
}

type StatusStream = ReceiverStream<Result<BatchStatus, Status>>;

/// Decode and convert one batch into something the writer takes.
type Convert = fn(&OtapBatch) -> Result<Batch, OtapError>;

impl OtapGrpc {
    pub fn new(ingest: IngestWriter, ingest_token: Option<Arc<String>>) -> Self {
        Self {
            ingest,
            ingest_token,
        }
    }

    /// The traces service with the compressions the Collector's exporter
    /// uses enabled.
    pub fn traces(self) -> ZstdArrowAlias<ArrowTracesServiceServer<Self>> {
        ZstdArrowAlias(
            ArrowTracesServiceServer::new(self)
                .accept_compressed(CompressionEncoding::Zstd)
                .accept_compressed(CompressionEncoding::Gzip),
        )
    }

    pub fn logs(self) -> ZstdArrowAlias<ArrowLogsServiceServer<Self>> {
        ZstdArrowAlias(
            ArrowLogsServiceServer::new(self)
                .accept_compressed(CompressionEncoding::Zstd)
                .accept_compressed(CompressionEncoding::Gzip),
        )
    }

    pub fn metrics(self) -> ZstdArrowAlias<ArrowMetricsServiceServer<Self>> {
        ZstdArrowAlias(
            ArrowMetricsServiceServer::new(self)
                .accept_compressed(CompressionEncoding::Zstd)
                .accept_compressed(CompressionEncoding::Gzip),
        )
    }

    fn start(
        &self,
        request: Request<Streaming<BatchArrowRecords>>,
        signal: &'static str,
        convert: Convert,
    ) -> Result<Response<StatusStream>, Status> {
        authorize(self.ingest_token.as_ref(), &request)?;
        let mut inbound = request.into_inner();
        let ingest = self.ingest.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tracing::info!(signal, "OTAP stream opened");
        tokio::spawn(async move {
            let mut consumer = StreamConsumer::default();
            loop {
                let records = match inbound.message().await {
                    Ok(Some(records)) => records,
                    Ok(None) => break,
                    Err(status) => {
                        tracing::warn!(signal, error = %status, "OTAP stream read failed");
                        break;
                    }
                };
                let status = handle_batch(&mut consumer, &ingest, convert, signal, records).await;
                if tx.send(Ok(status)).await.is_err() {
                    break;
                }
            }
            tracing::debug!(signal, "OTAP stream closed");
        });
        Ok(Response::new(ReceiverStream::new(rx)))
    }
}

async fn handle_batch(
    consumer: &mut StreamConsumer,
    ingest: &IngestWriter,
    convert: Convert,
    signal: &'static str,
    records: BatchArrowRecords,
) -> BatchStatus {
    let batch_id = records.batch_id;
    let batch = match consumer.consume(records).and_then(|b| convert(&b)) {
        Ok(batch) => batch,
        Err(e) => {
            tracing::warn!(signal, batch_id, error = %e, "OTAP batch rejected");
            return BatchStatus {
                batch_id,
                status_code: StatusCode::InvalidArgument as i32,
                status_message: e.to_string(),
            };
        }
    };
    match ingest.write(batch).await {
        Ok(()) => BatchStatus {
            batch_id,
            status_code: StatusCode::Ok as i32,
            status_message: String::new(),
        },
        Err(e) => {
            tracing::error!(signal, batch_id, error = %e, "OTAP write failed");
            BatchStatus {
                batch_id,
                status_code: StatusCode::Internal as i32,
                status_message: "telemetry write failed".into(),
            }
        }
    }
}

fn convert_traces(batch: &OtapBatch) -> Result<Batch, OtapError> {
    let request = to_trace_request(batch)?;
    Ok(Batch::Spans(tumult_otlp::trace_request_to_spans(&request)))
}

fn convert_logs(batch: &OtapBatch) -> Result<Batch, OtapError> {
    let request = to_logs_request(batch)?;
    Ok(Batch::Logs(tumult_otlp::logs_request_to_rows(
        &request,
        crate::now_ns(),
    )))
}

fn convert_metrics(batch: &OtapBatch) -> Result<Batch, OtapError> {
    let request = to_metrics_request(batch)?;
    let rows = tumult_otlp::metrics_request_to_rows(&request);
    if rows.rejected_data_points > 0 {
        // The OTLP paths report this as a partial success; BatchStatus has no
        // such field, so it is logged instead of silently dropped.
        tracing::warn!(
            rejected = rows.rejected_data_points,
            "OTAP metrics batch carried unsupported data points (exponential histograms, summaries)"
        );
    }
    Ok(Batch::Metrics(rows))
}

#[tonic::async_trait]
impl ArrowTracesService for OtapGrpc {
    type ArrowTracesStream = StatusStream;

    async fn arrow_traces(
        &self,
        request: Request<Streaming<BatchArrowRecords>>,
    ) -> Result<Response<Self::ArrowTracesStream>, Status> {
        self.start(request, "traces", convert_traces)
    }
}

#[tonic::async_trait]
impl ArrowLogsService for OtapGrpc {
    type ArrowLogsStream = StatusStream;

    async fn arrow_logs(
        &self,
        request: Request<Streaming<BatchArrowRecords>>,
    ) -> Result<Response<Self::ArrowLogsStream>, Status> {
        self.start(request, "logs", convert_logs)
    }
}

#[tonic::async_trait]
impl ArrowMetricsService for OtapGrpc {
    type ArrowMetricsStream = StatusStream;

    async fn arrow_metrics(
        &self,
        request: Request<Streaming<BatchArrowRecords>>,
    ) -> Result<Response<Self::ArrowMetricsStream>, Status> {
        self.start(request, "metrics", convert_metrics)
    }
}

/// Accepts the Collector exporter's default gRPC compression.
///
/// The `otelarrow` exporter compresses messages with zstd but announces
/// them as `grpc-encoding: zstdarrow<level>` (level 1 to 9, default 5); the
/// level only configures its encoder, each message is an ordinary zstd
/// frame. tonic knows a fixed set of encoding names and answers anything
/// else with `UNIMPLEMENTED`, so the name is rewritten to `zstd` before the
/// generated service sees it. Other unknown encodings still get tonic's
/// `UNIMPLEMENTED` status naming them.
#[derive(Clone)]
pub struct ZstdArrowAlias<S>(S);

impl<S: NamedService> NamedService for ZstdArrowAlias<S> {
    const NAME: &'static str = S::NAME;
}

impl<S, B> Service<http::Request<B>> for ZstdArrowAlias<S>
where
    S: Service<http::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, mut request: http::Request<B>) -> Self::Future {
        let aliased = request
            .headers()
            .get(GRPC_ENCODING)
            .is_some_and(|v| is_zstdarrow(v.as_bytes()));
        if aliased {
            tracing::debug!("zstdarrow gRPC encoding accepted as zstd");
            request
                .headers_mut()
                .insert(GRPC_ENCODING, http::HeaderValue::from_static("zstd"));
        }
        self.0.call(request)
    }
}

const GRPC_ENCODING: &str = "grpc-encoding";

/// `zstdarrow1` to `zstdarrow9`, the names the exporter registers.
fn is_zstdarrow(encoding: &[u8]) -> bool {
    matches!(encoding.strip_prefix(b"zstdarrow"), Some([b'1'..=b'9']))
}
