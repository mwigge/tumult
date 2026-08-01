use std::path::PathBuf;

use anyhow::{Context, Result};
use tumult_ingest::IngestWriter;
use tumult_lake::Store;

use crate::reports::parse_interval;

// ---------------------------------------------------------------------------
// Parquet lake export + retention (KRONIKA_LAKE_*)

/// `Some(24h)` by default; `KRONIKA_LAKE_INTERVAL` overrides, `0`/`off`
/// disables the lake job entirely.
pub(crate) fn lake_interval_from_env() -> Option<std::time::Duration> {
    let default = std::time::Duration::from_secs(86_400);
    let Ok(raw) = std::env::var("KRONIKA_LAKE_INTERVAL") else {
        return Some(default);
    };
    let raw = raw.trim();
    if raw.is_empty() || raw == "0" || raw.eq_ignore_ascii_case("off") {
        return None;
    }
    match parse_interval(raw) {
        Some(d) => Some(d),
        None => {
            tracing::warn!(
                value = raw,
                "invalid KRONIKA_LAKE_INTERVAL (want e.g. 30m or 24h); using 24h"
            );
            Some(default)
        }
    }
}

/// One export pass: fresh read-only reader (a long-lived reader pins its
/// snapshot), then retention deletes on the single writer when the policy
/// asks for them.
async fn run_lake_job(
    db_path: &std::path::Path,
    ingest: &IngestWriter,
    cfg: &tumult_lake::lake::LakeConfig,
) -> Result<()> {
    let (db, cfg2) = (db_path.to_path_buf(), cfg.clone());
    let report = tokio::task::spawn_blocking(move || -> Result<_> {
        let store = Store::at(&db);
        let reader = store.read_only().context("open store read-only")?;
        Ok(tumult_lake::lake::export(&reader, &cfg2)?)
    })
    .await??;
    let total: u64 = report.tables.iter().map(|t| t.rows).sum();
    tracing::info!(
        rows = total,
        files = report.tables.iter().map(|t| t.files.len()).sum::<usize>(),
        dir = %report.lake_dir,
        "lake export complete"
    );
    if cfg.retention_days > 0 {
        let cfg3 = cfg.clone();
        let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
        let slot2 = std::sync::Arc::clone(&slot);
        ingest
            .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
                *slot2.lock().unwrap_or_else(|e| e.into_inner()) = Some(
                    tumult_lake::lake::enforce_retention(writer, &cfg3).map_err(|e| e.to_string()),
                );
                Ok(())
            })))
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        match slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(Ok(deleted)) => {
                let total: u64 = deleted.values().sum();
                if total > 0 {
                    tracing::info!(rows = total, "lake retention reclaimed hot rows");
                }
            }
            Some(Err(e)) => anyhow::bail!("retention failed: {e}"),
            None => anyhow::bail!("retention did not run"),
        };
    }
    Ok(())
}

/// Spawn the lake scheduler: one export (+ optional retention) per interval.
/// Failures are logged and the schedule continues — the watermark makes the
/// next run retry from the last good state. The task holds an `IngestWriter`
/// clone, so it must stop on `shutdown` (dropping the clone) before the
/// daemon's drain waits for the writer channel to close; the returned handle
/// lets the caller wait for exactly that. An export already in flight runs
/// to completion first.
pub(crate) fn spawn_lake_scheduler(
    db_path: PathBuf,
    ingest: IngestWriter,
    cfg: tumult_lake::lake::LakeConfig,
    interval: std::time::Duration,
    shutdown: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tracing::info!(
        interval = ?interval,
        dir = %cfg.dir.display(),
        retention_days = cfg.retention_days,
        "lake export job enabled"
    );
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        // Skip the immediate first tick: export after one full interval.
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(e) = run_lake_job(&db_path, &ingest, &cfg).await {
                        tracing::warn!(error = %format!("{e:#}"), "lake export job failed");
                    }
                }
                () = shutdown.cancelled() => {
                    tracing::info!("lake export job stopping (shutdown)");
                    break;
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: shutdown must complete with the lake scheduler enabled.
    /// The scheduler holds an `IngestWriter` clone; before the cancellation
    /// token existed, that clone kept the writer channel open forever and the
    /// drain hung. Mirroring `serve`'s shutdown order (signal → wait →
    /// drain), each wait is timeout-guarded so a regression fails fast
    /// instead of hanging the test run.
    #[tokio::test]
    async fn shutdown_drains_with_lake_scheduler_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("k.duckdb");
        let store = Store::open(&db_path).unwrap();
        let (ingest, writer_task) = IngestWriter::spawn(store.writer().unwrap(), 8);
        let shutdown = tokio_util::sync::CancellationToken::new();
        let lake_task = spawn_lake_scheduler(
            db_path.clone(),
            ingest.clone(),
            tumult_lake::lake::LakeConfig::new(dir.path().join("lake"), 0),
            std::time::Duration::from_secs(3600),
            shutdown.clone(),
        );

        shutdown.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(10), lake_task)
            .await
            .expect("lake scheduler did not stop on shutdown")
            .expect("lake scheduler task panicked");
        drop(ingest);
        tokio::time::timeout(std::time::Duration::from_secs(10), writer_task)
            .await
            .expect("ingest writer drain hung — a sender is still alive")
            .expect("ingest writer task panicked");
    }
}
