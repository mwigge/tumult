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
//! | batch decoded but not convertible (multivariate metrics, orphan rows, duplicate ids, missing or malformed ids, a table of another signal, attribute limit) | `INVALID_ARGUMENT` (not retried) | stays open, until [`OtapLimits::max_rejected_batches`] of them: then closed with `INVALID_ARGUMENT` |
//! | payload not decodable, or wider or deeper than the schema caps | `INVALID_ARGUMENT` (not retried) | closed with `INVALID_ARGUMENT` |
//! | over a limit of the batch or of the stream itself (rows, decoded bytes, retained bytes per stream, dictionary deltas) | `INVALID_ARGUMENT` (not retried) | closed with `RESOURCE_EXHAUSTED` |
//! | over the client's own share of the retained bytes: its connection's ([`OtapLimits::max_connection_retained_bytes`]) or its peer's ([`OtapLimits::max_peer_retained_bytes`]) | `INVALID_ARGUMENT` (not retried: the client's other streams hold the bytes) | closed with `RESOURCE_EXHAUSTED` |
//! | over the batch deadline ([`OtapLimits::batch_deadline`]) | `INVALID_ARGUMENT` (not retried) | closed with `RESOURCE_EXHAUSTED` |
//! | over the retained bytes all streams share ([`OtapLimits::max_retained_bytes`]) | `UNAVAILABLE` (retried: other streams hold the bytes) | closed with `RESOURCE_EXHAUSTED` |
//! | stream read failed (oversized message, bad compression) | none | closed with the read error's status |
//! | no message for [`OtapLimits::stream_idle`], or the stream is older than [`OtapLimits::stream_lifetime`] | none | closed with `UNAVAILABLE` |
//! | the client does not take a status for [`OtapLimits::ack_timeout`] | none | closed |
//! | more streams than [`OtapLimits::max_streams`], than [`OtapLimits::max_streams_per_connection`] on one connection, or than [`OtapLimits::max_streams_per_peer`] from one peer | none | refused with `RESOURCE_EXHAUSTED` |
//! | decode task panicked (builds that unwind only) | none | closed with `INTERNAL` |
//!
//! The release profile sets `panic = "abort"`: there a panic ends the
//! daemon, and the last row does not exist. Nothing a client sends may
//! reach a panic, which is why the IPC messages are decoded by this crate's
//! own checked reader (see `ipc`) and not by arrow's.
//!
//! A payload that cannot be decoded may have carried a schema or dictionary
//! messages that later payloads build on, so the stream's IPC state is
//! unknown after it and the stream has to end; the exporter opens a new one,
//! which starts with fresh schemas.
//!
//! Why the batch is always acked before such a close: the exporter decides
//! what to do with a batch from its `BatchStatus` alone. `INVALID_ARGUMENT`
//! is permanent (the batch is dropped), `UNAVAILABLE` is retried, and
//! `RESOURCE_EXHAUSTED` is retried only with a `RetryInfo` detail, which a
//! `BatchStatus` cannot carry. The gRPC status a stream closes with is only
//! logged by the exporter, and every batch that has no status yet when the
//! stream ends is sent again on a new stream, without limit. A batch that
//! deterministically breaks a limit would therefore come back forever if
//! the stream closed without answering it (the Collector's own receiver
//! does that: it returns the decode error from its receive loop). So a
//! batch that can never succeed gets `INVALID_ARGUMENT` first, whatever
//! code then closes the stream, and only the shared retained-bytes pool,
//! which other clients' streams may free, gets the retried `UNAVAILABLE`.
//! A client over its own share of that pool is not told to retry: what is
//! in the way is what its own streams keep, and one connection could
//! otherwise fill the pool and have every other client's batches refused
//! (see `admission`). The one
//! close without a status is a failed stream read: a message tonic refuses
//! (larger than [`OtapLimits::max_message_bytes`], or badly compressed) has
//! no batch id to answer, so an exporter configured to send larger
//! messages than the receiver takes resends until its own retry settings
//! give up.
//!
//! Decoding runs on the blocking pool, at most
//! [`OtapLimits::max_concurrent_decodes`] batches at once, so a large batch
//! never stalls the async workers that serve other connections. The permit
//! is held until the batch is stored, so no more than that many converted
//! batches wait for the writer however many streams are open. Batches that
//! wait for a permit take turns by peer and by connection (see
//! `decode_gate`), so a client with many streams does not keep the others
//! waiting behind all of its batches.

use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};
use tonic::codegen::tokio_stream::wrappers::ReceiverStream;
use tonic::codegen::tokio_stream::{Stream, StreamExt};
use tonic::codegen::{http, CompressionEncoding, Service};
use tonic::server::NamedService;
use tonic::{Request, Response, Status, Streaming};

use super::admission::{Shares, StreamSlot};
use super::decode::StreamConsumer;
use super::decode_gate::{Client, DecodeGate};
use super::limits::{OtapLimits, RetainedPool};
use super::proto::arrow_logs_service_server::{ArrowLogsService, ArrowLogsServiceServer};
use super::proto::arrow_metrics_service_server::{ArrowMetricsService, ArrowMetricsServiceServer};
use super::proto::arrow_traces_service_server::{ArrowTracesService, ArrowTracesServiceServer};
use super::proto::{BatchArrowRecords, BatchStatus, StatusCode};
use super::{to_logs_request, to_metrics_request, to_trace_request, OtapBatch, OtapError};
use crate::grpc::authorize;
use crate::log_guard::{self, LogLimiter};
use crate::writer::{Batch, IngestWriter};

/// Shared state of the three services.
#[derive(Clone)]
pub struct OtapGrpc {
    ingest: IngestWriter,
    ingest_token: Option<Arc<String>>,
    limits: OtapLimits,
    /// One permit per open stream, across the three services.
    streams: Arc<Semaphore>,
    /// One permit per batch being decoded, converted or written.
    decodes: Arc<DecodeGate>,
    shares: Arc<Shares>,
    guards: Arc<Guards>,
}

pub(super) type StatusStream = ReceiverStream<Result<BatchStatus, Status>>;

/// Convert one decoded batch into something the writer takes.
pub(super) type Convert = fn(&OtapBatch) -> Result<Batch, OtapError>;

/// What the streams of one receiver share besides their permits.
pub(super) struct Guards {
    /// Bytes of schemas and dictionaries all streams keep between batches.
    pool: Arc<RetainedPool>,
    /// Limits the warnings a client can cause at will.
    refused: LogLimiter,
    rejected: LogLimiter,
}

impl Guards {
    pub fn new(limits: &OtapLimits) -> Arc<Self> {
        Arc::new(Self {
            pool: RetainedPool::new(limits.max_retained_bytes),
            refused: LogLimiter::warnings(),
            rejected: LogLimiter::warnings(),
        })
    }

    #[cfg(test)]
    pub fn pool(&self) -> &RetainedPool {
        &self.pool
    }
}

/// What one stream's task needs.
pub(super) struct StreamContext {
    pub ingest: IngestWriter,
    pub limits: OtapLimits,
    pub decodes: Arc<DecodeGate>,
    pub signal: &'static str,
    pub convert: Convert,
    pub guards: Arc<Guards>,
    /// The stream's place in the shares of its connection and its peer,
    /// held for as long as the stream lives. `None` without a peer
    /// address (an in-memory transport): the receiver's totals still hold.
    pub slot: Option<StreamSlot>,
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
            decodes: DecodeGate::new(limits.max_concurrent_decodes),
            shares: Shares::new(&limits),
            guards: Guards::new(&limits),
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

    fn refuse(&self, signal: &'static str, why: &'static str) -> Status {
        if let Some(suppressed) = self.guards.refused.allow() {
            tracing::warn!(
                signal,
                max_streams = self.limits.max_streams,
                max_streams_per_connection = self.limits.max_streams_per_connection,
                max_streams_per_peer = self.limits.max_streams_per_peer,
                suppressed,
                "OTAP stream refused: {why}"
            );
        }
        Status::resource_exhausted(why)
    }

    fn start(
        &self,
        request: Request<Streaming<BatchArrowRecords>>,
        signal: &'static str,
        convert: Convert,
    ) -> Result<Response<StatusStream>, Status> {
        authorize(self.ingest_token.as_ref(), &request)?;
        let slot = match request.remote_addr() {
            Some(peer) => Some(
                self.shares
                    .claim(peer)
                    .map_err(|refused| self.refuse(signal, refused.reason()))?,
            ),
            None => None,
        };
        let Ok(permit) = self.streams.clone().try_acquire_owned() else {
            return Err(self.refuse(signal, "too many open otel-arrow streams"));
        };
        tracing::info!(signal, "OTAP stream opened");
        let context = StreamContext {
            ingest: self.ingest.clone(),
            limits: self.limits,
            decodes: self.decodes.clone(),
            signal,
            convert,
            guards: self.guards.clone(),
            slot,
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
    let ack_timeout = context.limits.ack_timeout;
    let task = tokio::spawn(run_stream(inbound, context, tx.clone()));
    // A panic in the stream's task must not look like a clean end of stream.
    tokio::spawn(async move {
        let _permit = permit;
        if let Err(e) = task.await {
            let error = log_guard::clean_display(&e);
            tracing::error!(signal, error, "OTAP stream task failed");
            let failed = Err(Status::internal("otel-arrow stream task failed"));
            send(&tx, failed, ack_timeout, signal).await;
        }
        tracing::debug!(signal, "OTAP stream closed");
    });
    ReceiverStream::new(rx)
}

type Statuses = mpsc::Sender<Result<BatchStatus, Status>>;

/// Hand `item` to the client, waiting at most `wait` for it to make room.
/// `false` when the stream is over: the client is gone or does not read.
async fn send(
    tx: &Statuses,
    item: Result<BatchStatus, Status>,
    wait: Duration,
    signal: &'static str,
) -> bool {
    match tokio::time::timeout(wait, tx.send(item)).await {
        Ok(sent) => sent.is_ok(),
        Err(_) => {
            tracing::info!(
                signal,
                "OTAP stream closed: the client does not read its statuses"
            );
            false
        }
    }
}

async fn run_stream<S>(mut inbound: S, context: StreamContext, tx: Statuses)
where
    S: Stream<Item = Result<BatchArrowRecords, Status>> + Send + Unpin + 'static,
{
    let signal = context.signal;
    let limits = context.limits;
    let wait = limits.ack_timeout;
    // What the stream keeps is charged to its connection, its peer and
    // the receiver, in that order: the client's own shares answer first.
    let accounts = context
        .slot
        .iter()
        .flat_map(StreamSlot::retained)
        .chain([context.guards.pool.clone()])
        .collect();
    let mut consumer = StreamConsumer::with_accounts(limits, accounts);
    let opened = Instant::now();
    let mut rejected = 0usize;
    loop {
        let Some(left) = limits.stream_lifetime.checked_sub(opened.elapsed()) else {
            tracing::info!(signal, "OTAP stream closed: maximum lifetime reached");
            let over = Status::unavailable("otel-arrow stream reached its maximum lifetime");
            send(&tx, Err(over), wait, signal).await;
            return;
        };
        let next = tokio::time::timeout(left.min(limits.stream_idle), inbound.next()).await;
        let records = match next {
            Ok(Some(Ok(records))) => records,
            Ok(None) => return,
            Ok(Some(Err(status))) => {
                if let Some(suppressed) = context.guards.rejected.allow() {
                    let error = log_guard::clean_display(&status);
                    tracing::warn!(signal, error, suppressed, "OTAP stream read failed");
                }
                send(&tx, Err(status), wait, signal).await;
                return;
            }
            // The lifetime ran out first: the top of the loop says so.
            Err(_) if opened.elapsed() >= limits.stream_lifetime => continue,
            Err(_) => {
                tracing::info!(signal, "OTAP stream closed: idle");
                let idle = Status::unavailable("idle otel-arrow stream closed");
                send(&tx, Err(idle), wait, signal).await;
                return;
            }
        };
        let (returned, outcome) = handle_batch(consumer, &context, records).await;
        let (ack, close) = match outcome {
            Outcome::Ack(ack) => (Some(ack), None),
            Outcome::Reject(ack) => {
                rejected += 1;
                (Some(ack), too_many(rejected, &limits))
            }
            Outcome::AckThenClose(ack, status) => (Some(ack), Some(status)),
            Outcome::Close(status) => (None, Some(status)),
        };
        if let Some(ack) = ack {
            if !send(&tx, Ok(ack), wait, signal).await {
                return;
            }
        }
        if let Some(status) = close {
            send(&tx, Err(status), wait, signal).await;
            return;
        }
        let Some(returned) = returned else { return };
        consumer = returned;
    }
}

/// The status that ends a stream whose batches keep being rejected.
fn too_many(rejected: usize, limits: &OtapLimits) -> Option<Status> {
    (rejected >= limits.max_rejected_batches)
        .then(|| Status::invalid_argument("otel-arrow stream closed: too many rejected batches"))
}

/// How one batch ended.
enum Outcome {
    Ack(BatchStatus),
    /// The batch is refused; the stream goes on unless it has refused too
    /// many.
    Reject(BatchStatus),
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

/// The status that closes a stream after `e`, with the same text the
/// batch's ack carries. An [`OtapError`] holds no unbounded or unescaped
/// peer text (see `hostile_ipc`), so its message may go out as it is.
fn closing(e: &OtapError) -> Status {
    let message = format!("otel-arrow decode: {e}");
    if e.is_limit() {
        return Status::resource_exhausted(message);
    }
    Status::invalid_argument(message)
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
    let client = context
        .slot
        .as_ref()
        .map_or_else(Client::default, StreamSlot::client);
    let Some(permit) = context.decodes.request(client).permit().await else {
        return (
            None,
            Outcome::Close(Status::unavailable("otel-arrow receiver is shutting down")),
        );
    };
    let convert = context.convert;
    let decoding = tokio::task::spawn_blocking(move || {
        let decoded = match consumer.consume(records) {
            Ok(batch) => match convert(&batch) {
                Ok(rows) => Decoded::Batch(rows, batch.payloads(), batch.continued_payloads()),
                Err(e) => Decoded::Rejected(e),
            },
            Err(e) => Decoded::Undecodable(e),
        };
        (consumer, decoded, permit)
    });
    // The permit comes back with the result and is held until the batch is
    // stored: at most `max_concurrent_decodes` converted batches exist at
    // once, however many streams wait for the writer.
    let (consumer, decoded, _permit) = match decoding.await {
        Ok(done) => done,
        Err(e) => {
            // The panic message may quote payload bytes: it stays in the log.
            let error = log_guard::clean_display(&e);
            tracing::error!(signal, batch_id, error, "OTAP decode task failed");
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
        // Out of time: the same batch would run out again.
        Decoded::Rejected(e @ OtapError::Deadline { .. }) => {
            if let Some(suppressed) = context.guards.rejected.allow() {
                let error = log_guard::clean_display(&e);
                tracing::warn!(
                    signal,
                    batch_id,
                    error,
                    suppressed,
                    "OTAP batch over its deadline, closing the stream"
                );
            }
            let refused = ack(StatusCode::InvalidArgument, e.to_string());
            return (None, Outcome::AckThenClose(refused, closing(&e)));
        }
        Decoded::Rejected(e) => {
            if let Some(suppressed) = context.guards.rejected.allow() {
                // A string field is logged in quotes: what the peer put
                // in the error cannot pass for a field of the line.
                let error = log_guard::clean_display(&e);
                tracing::warn!(signal, batch_id, error, suppressed, "OTAP batch rejected");
            }
            let outcome = Outcome::Reject(ack(StatusCode::InvalidArgument, e.to_string()));
            return (Some(consumer), outcome);
        }
        Decoded::Undecodable(e) => {
            if let Some(suppressed) = context.guards.rejected.allow() {
                // Arrow's own error text may repeat what the peer sent.
                let cause = std::error::Error::source(&e).map(|c| log_guard::clean_display(&c));
                let error = log_guard::clean_display(&e);
                tracing::warn!(
                    signal,
                    batch_id,
                    error,
                    cause,
                    suppressed,
                    "OTAP batch undecodable, closing the stream"
                );
            }
            // A limit all streams share may have room again later: that one
            // is worth a retry. Everything else is the batch's own fault.
            let code = match e {
                OtapError::StreamState { shared: true, .. } => StatusCode::Unavailable,
                _ => StatusCode::InvalidArgument,
            };
            let outcome = Outcome::AckThenClose(ack(code, e.to_string()), closing(&e));
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
