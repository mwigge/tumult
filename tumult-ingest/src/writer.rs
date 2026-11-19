//! Single-writer channel: every ingest path funnels batches through one
//! bounded `tokio::mpsc` channel onto the store's single [`Writer`], giving
//! batching and backpressure for free.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use tokio::sync::{mpsc, oneshot};
use tumult_lake::{EntityEventRow, EntityLimits, EntityPurge, LogRow, SpanRow, Store, Writer};
use tumult_otlp::MetricRows;

use crate::entity_events::{self, EntityEvents};
use crate::error::IngestError;

/// An arbitrary write executed against the single [`Writer`] — used by
/// the manual-evidence lifecycle so API-triggered mutations ride the
/// same single-writer channel as telemetry (never a second connection).
pub type ExecFn = Box<dyn FnOnce(&Writer) -> Result<(), String> + Send>;

/// One batch of telemetry to persist.
pub enum Batch {
    Spans(Vec<SpanRow>),
    Logs(Vec<LogRow>),
    /// A logs export that carried entity events: the ordinary records and
    /// the events, stored in one transaction.
    LogsAndEntities {
        logs: Vec<LogRow>,
        entities: Vec<EntityEventRow>,
        /// How far the entity table may grow.
        limits: EntityLimits,
    },
    Metrics(MetricRows),
    /// See `ExecFn`.
    Exec(ExecFn),
}

/// What an operator asks to have removed from the entity table.
#[derive(Debug, Clone)]
pub struct EntityPurgeRequest {
    /// The environment whose entities go; empty for the entities that
    /// name none.
    pub environment: String,
    /// Only the entities of this type; `None` for every type.
    pub entity_type: Option<String>,
    /// Who asked, for the log.
    pub by: String,
}

/// What the writer task says about a batch it stored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Written {
    /// Entity events of the batch that were not stored because their
    /// entity would have been one too many.
    entities_refused: usize,
}

struct Envelope {
    batch: Batch,
    ack: oneshot::Sender<Result<Written, String>>,
}

/// Cheaply-cloneable handle to the background writer task.
#[derive(Clone)]
pub struct IngestWriter {
    tx: mpsc::Sender<Envelope>,
    /// `Some` when entity events are stored as entities.
    entities: Option<EntityEvents>,
}

impl IngestWriter {
    /// Spawn the writer task on the current tokio runtime. `buffer` bounds
    /// the channel; senders await (backpressure) when it is full.
    ///
    /// The returned `JoinHandle` finishes when every `IngestWriter` clone is
    /// dropped and the channel closes (graceful shutdown).
    pub fn spawn(writer: Writer, buffer: usize) -> (Self, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel::<Envelope>(buffer);
        let ingest = Self { tx, entities: None };
        (ingest, tokio::spawn(run_loop(rx, writer, None)))
    }

    /// Like [`IngestWriter::spawn`], but rebuilds the store connection when
    /// a persist fails *fatally*: DuckDB invalidates the connection on
    /// internal errors ("FATAL Error: …"), and every statement after that
    /// fails — without a reconnect the daemon keeps acking errors forever.
    /// The failed batch is NOT retried (partial-batch risk): its caller gets
    /// the error and tells the sender to send the batch again
    /// ([`crate::error::WriteFailed`]); subsequent batches land on the
    /// rebuilt writer.
    pub fn spawn_reconnect(
        writer: Writer,
        db_path: PathBuf,
        buffer: usize,
    ) -> (Self, tokio::task::JoinHandle<()>) {
        let (tx, rx) = mpsc::channel::<Envelope>(buffer);
        (
            Self { tx, entities: None },
            tokio::spawn(run_loop(rx, writer, Some(db_path))),
        )
    }

    /// Store entity events as entities (`Some`) or leave every log record
    /// a log record (`None`, the default).
    #[must_use]
    pub fn with_entity_events(mut self, entities: Option<EntityEvents>) -> Self {
        self.entities = entities;
        self
    }

    /// The batch a logs export becomes on this writer, whichever carrier
    /// delivered it.
    pub(crate) fn logs_batch(
        &self,
        request: &ExportLogsServiceRequest,
        received_at_ns: i64,
    ) -> Batch {
        entity_events::logs_export(self.entities.as_ref(), request, received_at_ns).0
    }

    /// Store a logs export and answer it as OTLP does: with the number of
    /// entity events that were accepted and not stored, and why, when
    /// there are any (`partial_success`).
    ///
    /// # Errors
    /// Returns [`IngestError::Channel`] if the writer task is gone or the
    /// store write failed; nothing of the export is stored then.
    pub(crate) async fn export_logs(
        &self,
        request: &ExportLogsServiceRequest,
    ) -> Result<ExportLogsServiceResponse, IngestError> {
        let (batch, dropped) =
            entity_events::logs_export(self.entities.as_ref(), request, crate::now_ns());
        let written = self.submit(batch).await?;
        Ok(entity_events::logs_response(
            &dropped,
            written.entities_refused,
        ))
    }

    /// Batches waiting in the channel for the writer task.
    #[cfg(test)]
    pub(crate) fn queued(&self) -> usize {
        self.tx.max_capacity() - self.tx.capacity()
    }

    /// Enqueue a batch and wait until it has been persisted (or failed).
    /// Awaiting the channel send is the backpressure mechanism.
    ///
    /// # Errors
    /// Returns [`IngestError::Channel`] if the writer task is gone or the
    /// store write failed.
    pub async fn write(&self, batch: Batch) -> Result<(), IngestError> {
        self.submit(batch).await.map(|_| ())
    }

    async fn submit(&self, batch: Batch) -> Result<Written, IngestError> {
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

    /// Remove the entities of one environment, with all their rows, on
    /// the writer task, and say so in the log, also when it fails.
    /// Entities that still report come back with their next report,
    /// without their history.
    ///
    /// # Errors
    /// Returns [`IngestError::Channel`] if the writer task is gone or the
    /// store refused the removal, in which case nothing was removed.
    pub async fn purge_entities(
        &self,
        request: EntityPurgeRequest,
    ) -> Result<EntityPurge, IngestError> {
        let asked = request.clone();
        let purged = self.run_purge(request).await;
        if let Err(error) = &purged {
            entity_events::purge_failed(&asked, &error.to_string());
        }
        purged
    }

    async fn run_purge(&self, request: EntityPurgeRequest) -> Result<EntityPurge, IngestError> {
        let slot = Arc::new(Mutex::new(None));
        let filled = Arc::clone(&slot);
        self.write(Batch::Exec(Box::new(move |writer: &Writer| {
            let purged = entity_events::purge(writer, &request)?;
            *filled.lock().unwrap_or_else(PoisonError::into_inner) = Some(purged);
            Ok(())
        })))
        .await?;
        let purged = slot.lock().unwrap_or_else(PoisonError::into_inner).take();
        purged.ok_or_else(|| IngestError::Channel("the purge did not run".into()))
    }

    /// Round-trip a no-op through the channel: proves the writer task is
    /// alive and processing (the daemon's `/healthz` liveness probe).
    ///
    /// # Errors
    /// Returns [`IngestError::Channel`] if the writer task is gone.
    pub async fn ping(&self) -> Result<(), IngestError> {
        self.write(Batch::Exec(Box::new(|_| Ok(())))).await
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
        Self { tx, entities: None }
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

fn apply(writer: &Writer, batch: Batch) -> Result<Written, IngestError> {
    let mut written = Written::default();
    match batch {
        Batch::Spans(rows) => writer.insert_spans(&rows)?,
        Batch::Logs(rows) => writer.insert_logs(&rows)?,
        Batch::LogsAndEntities {
            logs,
            entities,
            limits,
        } => {
            let applied = writer.insert_logs_and_entity_events(&logs, &entities, &limits)?;
            written.entities_refused = entity_events::after_write(writer, &applied);
        }
        Batch::Metrics(rows) => {
            writer.insert_metrics(&rows.sums, &rows.gauges, &rows.histograms)?;
        }
        Batch::Exec(f) => f(writer).map_err(IngestError::Channel)?,
    }
    Ok(written)
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

    fn log(body: &str) -> Batch {
        Batch::Logs(vec![tumult_lake::LogRow {
            ts_ns: 1,
            severity_text: "INFO".into(),
            body: body.into(),
            service_name: "writer-test".into(),
            ..Default::default()
        }])
    }

    fn log_bodies(store: &Store) -> serde_json::Value {
        let rows = store
            .read_only()
            .unwrap()
            .query_json_rows("SELECT body FROM logs ORDER BY body")
            .unwrap();
        serde_json::Value::Array(rows.into_iter().map(|row| row["body"].clone()).collect())
    }

    /// The store to reconnect to is a second file, so the test can see
    /// which connection each batch went through.
    #[tokio::test]
    async fn a_fatal_error_fails_its_batch_and_moves_later_ones_to_a_rebuilt_writer() {
        let d = tempfile::TempDir::new().unwrap();
        let first = Store::open(&d.path().join("first.duckdb")).unwrap();
        let rebuilt_path = d.path().join("rebuilt.duckdb");
        let rebuilt = Store::open(&rebuilt_path).unwrap();
        let (ingest, task) =
            IngestWriter::spawn_reconnect(first.writer().unwrap(), rebuilt_path, 4);

        ingest.write(log("before")).await.unwrap();
        let err = ingest
            .write(Batch::Exec(Box::new(|_| {
                Err("FATAL Error: database invalidated".to_string())
            })))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, IngestError::Channel(msg) if msg.contains("database invalidated")),
            "the failed batch is answered with its own error: {err:?}"
        );
        ingest.write(log("after")).await.unwrap();
        drop(ingest);
        task.await.unwrap();

        assert_eq!(log_bodies(&first), serde_json::json!(["before"]));
        assert_eq!(log_bodies(&rebuilt), serde_json::json!(["after"]));
    }

    #[tokio::test]
    async fn an_ordinary_error_keeps_the_writer_it_has() {
        let d = tempfile::TempDir::new().unwrap();
        let first = Store::open(&d.path().join("first.duckdb")).unwrap();
        let other_path = d.path().join("other.duckdb");
        let other = Store::open(&other_path).unwrap();
        let (ingest, task) = IngestWriter::spawn_reconnect(first.writer().unwrap(), other_path, 4);

        ingest
            .write(Batch::Exec(Box::new(|_| {
                Err("Constraint Error: duplicate key".to_string())
            })))
            .await
            .unwrap_err();
        ingest.write(log("after")).await.unwrap();
        drop(ingest);
        task.await.unwrap();

        assert_eq!(log_bodies(&first), serde_json::json!(["after"]));
        assert_eq!(log_bodies(&other), serde_json::json!([]));
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
            rejected_data_points: 0,
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

    #[tokio::test]
    async fn metrics_batch_is_stored_whole_or_not_at_all() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("k.duckdb")).unwrap();
        let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 4);
        // Make the last of the three inserts fail.
        ingest
            .write(Batch::Exec(Box::new(|writer| {
                writer
                    .execute("DROP TABLE metric_histograms", [])
                    .map(drop)
                    .map_err(|e| e.to_string())
            })))
            .await
            .unwrap();
        let rows = tumult_otlp::MetricRows {
            rejected_data_points: 0,
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
        ingest.write(Batch::Metrics(rows)).await.unwrap_err();
        // The sender retries the whole batch: nothing of it may be stored.
        let stored = store
            .read_only()
            .unwrap()
            .query_json_rows(
                "SELECT (SELECT count(*) FROM metric_sums) AS sums, \
                 (SELECT count(*) FROM metric_gauges) AS gauges",
            )
            .unwrap();
        assert_eq!(stored[0]["sums"], 0, "{stored:?}");
        assert_eq!(stored[0]["gauges"], 0, "{stored:?}");
    }
}
