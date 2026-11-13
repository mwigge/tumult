//! The three OTel-Arrow gRPC stream services.
//!
//! Each stream is handled sequentially: a `BatchArrowRecords` is decoded,
//! converted with `tumult-otlp` and written through the single-writer
//! channel before its `BatchStatus` goes back, so the writer's backpressure
//! reaches the exporter the same way it does for unary OTLP exports.
//!
//! What the exporter is told, and why (its reactions are those of the
//! Collector's `otelarrow` exporter):
//!
//! | Event | `BatchStatus` | Stream |
//! |---|---|---|
//! | stored | `OK` | stays open |
//! | write failed | `UNAVAILABLE` (the code the exporter retries) | stays open |
//! | batch decoded but not convertible (multivariate metrics, orphan rows, duplicate ids, attribute limit) | `INVALID_ARGUMENT` (not retried) | stays open |
//! | payload not decodable, or over a decode limit | `INVALID_ARGUMENT` (not retried) | closed with `INVALID_ARGUMENT`, or `RESOURCE_EXHAUSTED` for a limit |
//! | stream read failed (oversized message, bad compression) | none | closed with the read error's status |
//! | no message for [`OtapLimits::stream_idle`] | none | closed with `UNAVAILABLE` |
//! | decode task panicked | none | closed with `INTERNAL` |
//!
//! A payload that cannot be decoded may have carried a schema or dictionary
//! messages that later payloads build on, so the stream's IPC state is
//! unknown after it and the stream has to end; the exporter opens a new one,
//! which starts with fresh schemas. The Collector's own receiver ends the
//! stream the same way (it returns the decode error from its receive loop);
//! unlike it, the batch is acked first so the exporter drops that batch
//! instead of waiting for a status that never comes.
//!
//! Decoding runs on the blocking pool, at most
//! [`OtapLimits::max_concurrent_decodes`] batches at once, so a large batch
//! never stalls the async workers that serve other connections.

use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;
use tonic::codegen::tokio_stream::{Stream, StreamExt};
use tonic::codegen::{http, CompressionEncoding, Service};
use tonic::server::NamedService;
use tonic::{Request, Response, Status, Streaming};

use super::decode::StreamConsumer;
use super::limits::OtapLimits;
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
    limits: OtapLimits,
    /// One permit per open stream, across the three services.
    streams: Arc<Semaphore>,
    /// One permit per batch being decoded.
    decodes: Arc<Semaphore>,
}

pub(super) type StatusStream = ReceiverStream<Result<BatchStatus, Status>>;

/// Convert one decoded batch into something the writer takes.
pub(super) type Convert = fn(&OtapBatch) -> Result<Batch, OtapError>;

/// What one stream's task needs.
pub(super) struct StreamContext {
    pub ingest: IngestWriter,
    pub limits: OtapLimits,
    pub decodes: Arc<Semaphore>,
    pub signal: &'static str,
    pub convert: Convert,
}

impl OtapGrpc {
    /// The services under the default [`OtapLimits`].
    pub fn new(ingest: IngestWriter, ingest_token: Option<Arc<String>>) -> Self {
        Self::with_limits(ingest, ingest_token, OtapLimits::default())
    }

    pub fn with_limits(
        ingest: IngestWriter,
        ingest_token: Option<Arc<String>>,
        limits: OtapLimits,
    ) -> Self {
        Self {
            ingest,
            ingest_token,
            limits,
            streams: Arc::new(Semaphore::new(limits.max_streams)),
            decodes: Arc::new(Semaphore::new(limits.max_concurrent_decodes)),
        }
    }

    /// The traces service: the compressions the Collector's exporter uses
    /// enabled, and messages capped at [`OtapLimits::max_message_bytes`]
    /// (the cap holds for the compressed frame and for what it inflates to).
    pub fn traces(self) -> ZstdArrowAlias<ArrowTracesServiceServer<Self>> {
        let max = self.limits.max_message_bytes;
        ZstdArrowAlias(
            ArrowTracesServiceServer::new(self)
                .accept_compressed(CompressionEncoding::Zstd)
                .accept_compressed(CompressionEncoding::Gzip)
                .max_decoding_message_size(max),
        )
    }

    /// The logs service, configured like [`Self::traces`].
    pub fn logs(self) -> ZstdArrowAlias<ArrowLogsServiceServer<Self>> {
        let max = self.limits.max_message_bytes;
        ZstdArrowAlias(
            ArrowLogsServiceServer::new(self)
                .accept_compressed(CompressionEncoding::Zstd)
                .accept_compressed(CompressionEncoding::Gzip)
                .max_decoding_message_size(max),
        )
    }

    /// The metrics service, configured like [`Self::traces`].
    pub fn metrics(self) -> ZstdArrowAlias<ArrowMetricsServiceServer<Self>> {
        let max = self.limits.max_message_bytes;
        ZstdArrowAlias(
            ArrowMetricsServiceServer::new(self)
                .accept_compressed(CompressionEncoding::Zstd)
                .accept_compressed(CompressionEncoding::Gzip)
                .max_decoding_message_size(max),
        )
    }

    fn start(
        &self,
        request: Request<Streaming<BatchArrowRecords>>,
        signal: &'static str,
        convert: Convert,
    ) -> Result<Response<StatusStream>, Status> {
        authorize(self.ingest_token.as_ref(), &request)?;
        let Ok(permit) = self.streams.clone().try_acquire_owned() else {
            tracing::warn!(
                signal,
                max_streams = self.limits.max_streams,
                "OTAP stream refused: too many open streams"
            );
            return Err(Status::resource_exhausted(
                "too many open otel-arrow streams",
            ));
        };
        tracing::info!(signal, "OTAP stream opened");
        let context = StreamContext {
            ingest: self.ingest.clone(),
            limits: self.limits,
            decodes: self.decodes.clone(),
            signal,
            convert,
        };
        Ok(Response::new(respond(
            request.into_inner(),
            context,
            Some(permit),
        )))
    }
}

/// Serve one stream: every inbound batch is answered on the returned stream,
/// which ends with an `Err` status whenever the stream did not end cleanly.
/// `permit` is held until the stream's task is gone.
pub(super) fn respond<S>(
    inbound: S,
    context: StreamContext,
    permit: Option<OwnedSemaphorePermit>,
) -> StatusStream
where
    S: Stream<Item = Result<BatchArrowRecords, Status>> + Send + Unpin + 'static,
{
    let (tx, rx) = mpsc::channel(4);
    let signal = context.signal;
    let task = tokio::spawn(run_stream(inbound, context, tx.clone()));
    // A panic in the stream's task must not look like a clean end of stream.
    tokio::spawn(async move {
        let _permit = permit;
        if let Err(e) = task.await {
            tracing::error!(signal, error = %e, "OTAP stream task failed");
            let _ = tx
                .send(Err(Status::internal("otel-arrow stream task failed")))
                .await;
        }
        tracing::debug!(signal, "OTAP stream closed");
    });
    ReceiverStream::new(rx)
}

async fn run_stream<S>(
    mut inbound: S,
    context: StreamContext,
    tx: mpsc::Sender<Result<BatchStatus, Status>>,
) where
    S: Stream<Item = Result<BatchArrowRecords, Status>> + Send + Unpin + 'static,
{
    let signal = context.signal;
    let mut consumer = StreamConsumer::new(context.limits);
    loop {
        let next = tokio::time::timeout(context.limits.stream_idle, inbound.next()).await;
        let records = match next {
            Ok(Some(Ok(records))) => records,
            Ok(None) => return,
            Ok(Some(Err(status))) => {
                tracing::warn!(signal, error = %status, "OTAP stream read failed");
                let _ = tx.send(Err(status)).await;
                return;
            }
            Err(_) => {
                tracing::info!(signal, "OTAP stream closed: idle");
                let _ = tx
                    .send(Err(Status::unavailable("idle otel-arrow stream closed")))
                    .await;
                return;
            }
        };
        let (returned, outcome) = handle_batch(consumer, &context, records).await;
        let (ack, close) = match outcome {
            Outcome::Ack(ack) => (Some(ack), None),
            Outcome::AckThenClose(ack, status) => (Some(ack), Some(status)),
            Outcome::Close(status) => (None, Some(status)),
        };
        if let Some(ack) = ack {
            if tx.send(Ok(ack)).await.is_err() {
                return;
            }
        }
        if let Some(status) = close {
            let _ = tx.send(Err(status)).await;
            return;
        }
        let Some(returned) = returned else { return };
        consumer = returned;
    }
}

/// How one batch ended.
enum Outcome {
    Ack(BatchStatus),
    /// The batch is refused and the stream's IPC state is lost with it.
    AckThenClose(BatchStatus, Status),
    Close(Status),
}

/// What the blocking decode hands back.
enum Decoded {
    /// Rows, tables in the batch, tables that continued an earlier schema.
    Batch(Batch, usize, usize),
    /// Decoded, but not convertible: the stream's IPC state is intact.
    Rejected(OtapError),
    /// Not decodable: the stream cannot go on.
    Undecodable(OtapError),
}

async fn handle_batch(
    mut consumer: StreamConsumer,
    context: &StreamContext,
    records: BatchArrowRecords,
) -> (Option<StreamConsumer>, Outcome) {
    let signal = context.signal;
    let batch_id = records.batch_id;
    let ack = |code: StatusCode, message: String| BatchStatus {
        batch_id,
        status_code: code as i32,
        status_message: message,
    };
    let Ok(permit) = context.decodes.clone().acquire_owned().await else {
        return (
            None,
            Outcome::Close(Status::unavailable("otel-arrow receiver is shutting down")),
        );
    };
    let convert = context.convert;
    let decoding = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let decoded = match consumer.consume(records) {
            Ok(batch) => match convert(&batch) {
                Ok(rows) => Decoded::Batch(rows, batch.payloads(), batch.continued_payloads()),
                Err(e) => Decoded::Rejected(e),
            },
            Err(e) => Decoded::Undecodable(e),
        };
        (consumer, decoded)
    });
    let (consumer, decoded) = match decoding.await {
        Ok(done) => done,
        Err(e) => {
            // The panic message may quote payload bytes: it stays in the log.
            tracing::error!(signal, batch_id, error = %e, "OTAP decode task failed");
            return (
                None,
                Outcome::Close(Status::internal("otel-arrow decode task failed")),
            );
        }
    };
    let batch = match decoded {
        Decoded::Batch(batch, payloads, continued) => {
            tracing::debug!(signal, batch_id, payloads, continued, "OTAP batch decoded");
            batch
        }
        Decoded::Rejected(e) => {
            tracing::warn!(signal, batch_id, error = %e, "OTAP batch rejected");
            let outcome = Outcome::Ack(ack(StatusCode::InvalidArgument, e.to_string()));
            return (Some(consumer), outcome);
        }
        Decoded::Undecodable(e) => {
            tracing::warn!(
                signal,
                batch_id,
                error = %e,
                cause = std::error::Error::source(&e).map(tracing::field::display),
                "OTAP batch undecodable, closing the stream"
            );
            let message = format!("otel-arrow decode: {e}");
            let status = if e.is_limit() {
                Status::resource_exhausted(message)
            } else {
                Status::invalid_argument(message)
            };
            let outcome =
                Outcome::AckThenClose(ack(StatusCode::InvalidArgument, e.to_string()), status);
            return (None, outcome);
        }
    };
    let outcome = match context.ingest.write(batch).await {
        Ok(()) => Outcome::Ack(ack(StatusCode::Ok, String::new())),
        Err(e) => {
            tracing::error!(signal, batch_id, error = %e, "OTAP write failed");
            // Nothing is wrong with the batch: UNAVAILABLE makes the
            // exporter send it again.
            Outcome::Ack(ack(
                StatusCode::Unavailable,
                "telemetry write failed".into(),
            ))
        }
    };
    (Some(consumer), outcome)
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
