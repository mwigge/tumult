//! Single-writer channel: every ingest path funnels batches through one
//! bounded `tokio::mpsc` channel onto the store's single [`Writer`], giving
//! batching and backpressure for free.

use tokio::sync::{mpsc, oneshot};
use tumult_lake::{LogRow, SpanRow, Writer};
use tumult_otlp::MetricRows;

use crate::error::IngestError;

/// An arbitrary write executed against the single [`Writer`] — used by
/// the manual-evidence lifecycle so API-triggered mutations ride the
/// same single-writer channel as telemetry (never a second connection).
pub type ExecFn = Box<dyn FnOnce(&Writer) -> Result<(), String> + Send>;

/// One batch of telemetry to persist.
pub enum Batch {
    Spans(Vec<SpanRow>),
    Logs(Vec<LogRow>),
    Metrics(MetricRows),
    /// See [`ExecFn`].
    Exec(ExecFn),
}

struct Envelope {
    batch: Batch,
    ack: oneshot::Sender<Result<(), String>>,
}

/// Cheaply-cloneable handle to the background writer task.
#[derive(Clone)]
pub struct IngestWriter {
    tx: mpsc::Sender<Envelope>,
}

impl IngestWriter {
    /// Spawn the writer task on the current tokio runtime. `buffer` bounds
    /// the channel; senders await (backpressure) when it is full.
    ///
    /// The returned `JoinHandle` finishes when every `IngestWriter` clone is
    /// dropped and the channel closes (graceful shutdown).
    pub fn spawn(writer: Writer, buffer: usize) -> (Self, tokio::task::JoinHandle<()>) {
        let (tx, mut rx) = mpsc::channel::<Envelope>(buffer);
        let task = tokio::spawn(async move {
            while let Some(envelope) = rx.recv().await {
                let result = apply(&writer, envelope.batch).map_err(|e| e.to_string());
                if let Err(e) = &result {
                    tracing::error!(error = %e, "failed to persist ingest batch");
                }
                // The sender may be gone (caller timed out); ignore.
                let _ = envelope.ack.send(result);
            }
            tracing::info!("ingest writer channel closed; writer task exiting");
        });
        (Self { tx }, task)
    }

    /// Enqueue a batch and wait until it has been persisted (or failed).
    /// Awaiting the channel send is the backpressure mechanism.
    ///
    /// # Errors
    /// Returns [`IngestError::Channel`] if the writer task is gone or the
    /// store write failed.
    pub async fn write(&self, batch: Batch) -> Result<(), IngestError> {
        let (ack_tx, ack_rx) = oneshot::channel();
        self.tx
            .send(Envelope { batch, ack: ack_tx })
            .await
            .map_err(|_| IngestError::Channel("writer task stopped".into()))?;
        ack_rx
            .await
            .map_err(|_| IngestError::Channel("writer task dropped the ack".into()))?
            .map_err(IngestError::Channel)
    }
}

fn apply(writer: &Writer, batch: Batch) -> Result<(), IngestError> {
    match batch {
        Batch::Spans(rows) => writer.insert_spans(&rows)?,
        Batch::Logs(rows) => writer.insert_logs(&rows)?,
        Batch::Metrics(rows) => {
            writer.insert_metric_sums(&rows.sums)?;
            writer.insert_metric_gauges(&rows.gauges)?;
            writer.insert_metric_histograms(&rows.histograms)?;
        }
        Batch::Exec(f) => f(writer).map_err(IngestError::Channel)?,
    }
    Ok(())
}
