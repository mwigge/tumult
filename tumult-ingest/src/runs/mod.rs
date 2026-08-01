//! Bounded in-process experiment run queue for tumultd.
//!
//! [`RunQueue`] accepts validated definitions from the API, persists every
//! state transition through the daemon's single-writer channel (schema v5
//! `runs` / `run_audit`), and executes runs on a fixed pool of worker tasks
//! via [`tumult_core::runner::run_experiment`]. Both the worker count and
//! the waiting-queue depth are bounded; overload is rejected, never queued
//! unboundedly. Each running experiment holds a
//! [`tokio_util::sync::CancellationToken`] — tumult-core's e-stop primitive —
//! so `POST /api/runs/{id}/stop` cancels mid-method and the runner's own
//! rollback path unwinds the fault.
//!
//! [`reconcile_orphans`] runs at daemon startup: runs left active by a
//! previous process lifetime are marked `orphaned`, their rollbacks are
//! attempted via [`tumult_core::runner::run_orphan_rollback`], and the
//! outcome is recorded in the run audit trail.

mod queue;
mod reconcile;
#[cfg(test)]
mod tests;
mod worker;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tokio_util::sync::CancellationToken;
use tumult_lake::{Store, Writer};

use crate::{Batch, IngestWriter};

pub use queue::{
    prepare_run, DispatchError, EnqueueError, ExecutorFactory, RunQueue, RunQueueConfig,
    RunRequest, StopError,
};
pub use reconcile::reconcile_orphans;

struct Shared {
    db_path: PathBuf,
    ingest: IngestWriter,
    /// Cancellation tokens of runs executing in this process, by run id.
    tokens: Mutex<HashMap<String, CancellationToken>>,
    /// Signals the background sweeper to stop on daemon shutdown, releasing
    /// the `IngestWriter` clone inside this struct so the ingest channel
    /// can close and the writer drain can complete.
    shutdown: CancellationToken,
}

struct WorkItem {
    run_id: String,
    request: RunRequest,
    /// The approved canonical pin for gated runs (`None` for T0 direct
    /// enqueues): re-verified by the worker before the run starts (T10).
    approval_pin: Option<String>,
    /// Held until the worker dequeues: bounds the waiting queue.
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Run one state-mutating closure on the single writer (same channel the
/// telemetry batches ride, so run-state writes interleave safely).
async fn exec_write(
    ingest: &IngestWriter,
    f: impl FnOnce(&Writer) -> Result<(), String> + Send + 'static,
) -> Result<(), String> {
    ingest
        .write(Batch::Exec(Box::new(f)))
        .await
        .map_err(|e| e.to_string())
}

/// Current state of one run, read on a fresh read-only connection.
fn read_run_state(db_path: &Path, run_id: &str) -> Option<String> {
    let reader = Store::at(db_path).read_only().ok()?;
    let run = reader.run_get(run_id).ok()??;
    run["state"].as_str().map(str::to_string)
}

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}
