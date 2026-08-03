//! Single-writer channel: every ingest path funnels batches through one
//! bounded `tokio::mpsc` channel onto the store's single [`Writer`], giving
//! batching and backpressure for free.

use std::path::{Path, PathBuf};

use tokio::sync::{mpsc, oneshot};
use tumult_lake::{LogRow, SpanRow, Store, Writer};
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
        let (tx, rx) = mpsc::channel::<Envelope>(buffer);
        (Self { tx }, tokio::spawn(run_loop(rx, writer, None)))
    }

    /// Like [`IngestWriter::spawn`], but rebuilds the store connection when
    /// a persist fails *fatally*: DuckDB invalidates the connection on
    /// internal errors ("FATAL Error: …"), and every statement after that
    /// fails — without a reconnect the daemon keeps acking errors forever.
    /// The failed batch is NOT retried (partial-batch risk): its caller gets
    /// the error; subsequent batches land on the rebuilt writer.
    pub fn spawn_reconnect(
        writer: Writer,
        db_path: PathBuf,
        buffer: usize,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel::<Envelope>(buffer);
        (
            Self { tx },
            tokio::spawn(run_loop(rx, writer, Some(db_path))),
        )
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

#[cfg(test)]
impl IngestWriter {
    /// A writer whose task is gone (the channel receiver was dropped):
    /// every `write` fails with `IngestError::Channel`, deterministically —
    /// the state the daemon is in after the writer task died.
    pub(crate) fn stopped_for_test() -> Self {
        let (tx, rx) = mpsc::channel::<Envelope>(1);
        drop(rx);
        Self { tx }
    }
}

/// The writer task's receive loop. When `db_path` is set, a FATAL persist
/// error triggers a bounded reconnect ([`reconnect`]) and the task swaps to
/// the rebuilt writer instead of poisoning every subsequent batch.
async fn run_loop(mut rx: mpsc::Receiver<Envelope>, mut writer: Writer, db_path: Option<PathBuf>) {
    while let Some(envelope) = rx.recv().await {
        let result = apply(&writer, envelope.batch).map_err(|e| e.to_string());
        if let Err(e) = &result {
            tracing::error!(error = %e, "failed to persist ingest batch");
            if let Some(path) = db_path.as_ref().filter(|_| should_reconnect(e)) {
                match reconnect(path, 3).await {
                    Ok(rebuilt) => {
                        tracing::warn!("ingest writer reconnected after fatal store error");
                        writer = rebuilt;
                    }
                    Err(reconnect_err) => {
                        tracing::error!(error = %reconnect_err, "ingest writer reconnect failed");
                    }
                }
            }
        }
        // The sender may be gone (caller timed out); ignore.
        let _ = envelope.ack.send(result);
    }
    tracing::info!("ingest writer channel closed; writer task exiting");
}

/// Whether a persist error means the underlying connection is dead and must
/// be rebuilt: DuckDB invalidates the connection on internal errors
/// ("FATAL Error: …"), after which every subsequent statement fails.
fn should_reconnect(error: &str) -> bool {
    error.contains("FATAL")
}

/// Rebuild the store writer from `path` with bounded retries and linear
/// backoff. The failed batch that triggered this is not retried — a new
/// connection re-attaches to the live DuckDB instance (same process) or
/// reopens the file, so later batches can proceed.
async fn reconnect(path: &Path, attempts: u32) -> Result<Writer, String> {
    reconnect_with(
        || Store::at(path).writer().map_err(|e| e.to_string()),
        attempts,
    )
    .await
}

/// The retry/backoff core of [`reconnect`], generic over the writer factory
/// so tests can drive the failure paths without a corrupt store.
async fn reconnect_with(
    make_writer: impl Fn() -> Result<Writer, String>,
    attempts: u32,
) -> Result<Writer, String> {
    let mut last_err = String::from("no reconnect attempted");
    for attempt in 1..=attempts {
        match make_writer() {
            Ok(writer) => return Ok(writer),
            Err(e) => {
                last_err = e;
                tracing::warn!(attempt, error = %last_err, "ingest writer reconnect attempt failed");
                tokio::time::sleep(std::time::Duration::from_millis(50 * u64::from(attempt))).await;
            }
        }
    }
    Err(last_err)
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::*;

    #[test]
    fn fatal_errors_trigger_reconnect_others_do_not() {
        assert!(should_reconnect(
            "IO Error: FATAL Error: Failed to delete all rows from index"
        ));
        assert!(should_reconnect("FATAL Error: database invalidated"));
        assert!(!should_reconnect("Constraint Error: duplicate key"));
        assert!(!should_reconnect(
            "fatal: lowercase is not the DuckDB marker"
        ));
    }

    #[tokio::test]
    async fn reconnect_succeeds_after_transient_failures() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("k.duckdb")).unwrap();
        let failures = AtomicU32::new(2);
        let writer = reconnect_with(
            || {
                if failures.fetch_sub(1, Ordering::SeqCst) > 0 {
                    Err("still down".into())
                } else {
                    store.writer().map_err(|e| e.to_string())
                }
            },
            3,
        )
        .await
        .unwrap();
        // The rebuilt writer is a working connection.
        assert!(writer.schema_version().unwrap() > 0);
    }

    #[tokio::test]
    async fn reconnect_gives_up_after_bounded_attempts() {
        // Writer is not Debug, so no unwrap_err.
        let err = match reconnect_with(|| Err::<Writer, _>("always down".into()), 3).await {
            Ok(_) => panic!("reconnect should have failed"),
            Err(e) => e,
        };
        assert_eq!(err, "always down");
    }

    #[tokio::test]
    async fn spawn_reconnect_persists_batches_like_spawn() {
        let d = tempfile::TempDir::new().unwrap();
        let db_path = d.path().join("k.duckdb");
        let store = Store::open(&db_path).unwrap();
        let (ingest, task) =
            IngestWriter::spawn_reconnect(store.writer().unwrap(), db_path.clone(), 4);
        ingest.write(Batch::Logs(vec![])).await.unwrap();
        drop(ingest);
        task.await.unwrap();
        // A real FATAL error cannot be triggered deterministically in a
        // test; the predicate and retry core above cover the branches.
    }

    #[tokio::test]
    async fn write_fails_when_the_writer_task_is_gone() {
        let err = IngestWriter::stopped_for_test()
            .write(Batch::Logs(vec![]))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, IngestError::Channel(msg) if msg.contains("writer task stopped")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn exec_batch_runs_the_closure_on_the_single_writer() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("k.duckdb")).unwrap();
        let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 4);
        ingest
            .write(Batch::Exec(Box::new(|writer| {
                writer
                    .insert_run(&tumult_lake::NewRun {
                        id: "run-exec".into(),
                        registry_id: "reg-1".into(),
                        params_json: None,
                        queued_at_ns: 1,
                        actor: None,
                    })
                    .map_err(|e| e.to_string())
            })))
            .await
            .unwrap();
        let run = store
            .read_only()
            .unwrap()
            .run_get("run-exec")
            .unwrap()
            .unwrap();
        assert_eq!(
            run["state"],
            serde_json::json!(tumult_lake::run_state::QUEUED)
        );

        // A failing closure surfaces its message through the channel error.
        let err = ingest
            .write(Batch::Exec(Box::new(|_writer| Err("boom".to_string()))))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, IngestError::Channel(msg) if msg.ends_with("boom")),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn metrics_batch_persists_sums_gauges_and_histograms() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("k.duckdb")).unwrap();
        let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 4);
        let rows = tumult_otlp::MetricRows {
            sums: vec![tumult_lake::MetricSumRow {
                ts_ns: 1,
                metric_name: "demo.sum".into(),
                value: 3.0,
                ..Default::default()
            }],
            gauges: vec![tumult_lake::MetricGaugeRow {
                ts_ns: 2,
                metric_name: "demo.gauge".into(),
                value: 0.5,
                ..Default::default()
            }],
            histograms: vec![tumult_lake::MetricHistogramRow {
                ts_ns: 3,
                metric_name: "demo.hist".into(),
                count: 2,
                sum: 10.0,
                bucket_counts: vec![1, 1],
                explicit_bounds: vec![5.0],
                ..Default::default()
            }],
        };
        ingest.write(Batch::Metrics(rows)).await.unwrap();
        let reader = store.read_only().unwrap();
        let sums = reader
            .query_json_rows("SELECT value FROM metric_sums")
            .unwrap();
        assert_eq!(sums[0]["value"], serde_json::json!(3.0));
        let gauges = reader
            .query_json_rows("SELECT value FROM metric_gauges")
            .unwrap();
        assert_eq!(gauges[0]["value"], serde_json::json!(0.5));
        let hists = reader
            .query_json_rows("SELECT count FROM metric_histograms")
            .unwrap();
        assert_eq!(hists[0]["count"], serde_json::json!(2));
    }
}
