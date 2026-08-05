//! Run-system retention: `runs` and `run_audit` grow monotonically, so a
//! tick task chained like the other daemon schedulers deletes terminal runs
//! — and their audit trails — older than `TUMULTD_RUN_RETENTION_DAYS`
//! (default 90). Deletes ride the single-writer channel like every other
//! mutation; active runs are never touched.

use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tumult_lake::run_state;

use crate::{Batch, IngestWriter};

/// Hot-store retention for terminal runs in days, from
/// `TUMULTD_RUN_RETENTION_DAYS` (default 90, minimum 1); invalid values
/// fall back to the default.
#[must_use]
pub fn retention_days_from_env() -> u64 {
    std::env::var("TUMULTD_RUN_RETENTION_DAYS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&d| d > 0)
        .unwrap_or(90)
}

/// The sweep interval from `TUMULTD_RUN_RETENTION_TICK_S` (default 3600s,
/// minimum 1s); invalid values fall back to the default.
#[must_use]
pub fn tick_from_env() -> Duration {
    std::env::var("TUMULTD_RUN_RETENTION_TICK_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map_or_else(|| Duration::from_secs(3600), Duration::from_secs)
}

/// Spawn the retention sweeper (same shutdown contract as the other daemon
/// background tasks: cancel the token and await before draining the
/// writer).
pub fn spawn_run_retention(
    ingest: IngestWriter,
    tick: Duration,
    retention_days: u64,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = sweep_expired_runs(&ingest, retention_days).await {
                        tracing::warn!(error = %e, "run retention sweep failed");
                    }
                }
                () = shutdown.cancelled() => {
                    tracing::info!("run retention sweeper exiting (shutdown)");
                    break;
                }
            }
        }
    })
}

/// One sweep: delete terminal runs whose `ended_at_ns` is older than the
/// cutoff, together with their audit rows (audit first, so a crash between
/// the two deletes never leaves a run whose trail is gone).
///
/// # Errors
/// Returns an error if the write fails.
pub async fn sweep_expired_runs(ingest: &IngestWriter, retention_days: u64) -> Result<(), String> {
    let cutoff = crate::now_ns() - (retention_days as i64) * 86_400 * 1_000_000_000;
    let terminal = run_state::TERMINAL
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(", ");
    ingest
        .write(Batch::Exec(Box::new(move |writer| {
            let audit_deleted = writer
                .execute(
                    &format!(
                        "DELETE FROM run_audit WHERE run_id IN \
                         (SELECT id FROM runs WHERE state IN ({terminal}) \
                           AND ended_at_ns IS NOT NULL AND ended_at_ns < {cutoff})"
                    ),
                    [],
                )
                .map_err(|e| e.to_string())?;
            let runs_deleted = writer
                .execute(
                    &format!(
                        "DELETE FROM runs WHERE state IN ({terminal}) \
                         AND ended_at_ns IS NOT NULL AND ended_at_ns < {cutoff}"
                    ),
                    [],
                )
                .map_err(|e| e.to_string())?;
            if runs_deleted > 0 || audit_deleted > 0 {
                tracing::info!(
                    runs_deleted,
                    audit_deleted,
                    retention_days,
                    "run retention sweep reclaimed terminal runs"
                );
            }
            Ok(())
        })))
        .await
        .map_err(|e| e.to_string())
}
