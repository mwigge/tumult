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

/// One export pass. Reject retention before writing: reports still require
/// the complete hot-store history.
async fn run_lake_job(
    db_path: &std::path::Path,
    _ingest: &IngestWriter,
    cfg: &tumult_lake::lake::LakeConfig,
) -> Result<()> {
    tumult_ingest::retention::validate_policy(
        "KRONIKA_RETENTION_DAYS",
        Some(&cfg.retention_days.to_string()),
    )
    .map_err(anyhow::Error::msg)?;
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
    Ok(())
}

/// Spawn the lake scheduler: one manifest snapshot export per interval.
/// Failures are logged; content-addressed snapshots make retries idempotent. The task holds an `IngestWriter`
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

    #[test]
    fn lake_interval_from_env_defaults_to_daily_and_honours_overrides() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let daily = std::time::Duration::from_secs(86_400);
        std::env::remove_var("KRONIKA_LAKE_INTERVAL");
        assert_eq!(lake_interval_from_env(), Some(daily));
        for off in ["", "0", "off", "OFF"] {
            std::env::set_var("KRONIKA_LAKE_INTERVAL", off);
            assert_eq!(lake_interval_from_env(), None, "{off:?} must disable");
        }
        std::env::set_var("KRONIKA_LAKE_INTERVAL", "30m");
        assert_eq!(
            lake_interval_from_env(),
            Some(std::time::Duration::from_secs(1_800))
        );
        // Invalid values fall back to the default rather than disabling the job.
        std::env::set_var("KRONIKA_LAKE_INTERVAL", "bogus");
        assert_eq!(lake_interval_from_env(), Some(daily));
        std::env::remove_var("KRONIKA_LAKE_INTERVAL");
    }

    /// Import `csv` into a fresh store, then hand the writer to the ingest
    /// channel to model a live daemon writer.
    fn store_with_spans(
        dir: &std::path::Path,
        csv: &str,
    ) -> (PathBuf, Store, IngestWriter, tokio::task::JoinHandle<()>) {
        let db_path = dir.join("lake.duckdb");
        let store = Store::open(&db_path).unwrap();
        let csv_path = dir.join("spans.csv");
        std::fs::write(&csv_path, csv).unwrap();
        {
            let writer = store.writer().unwrap();
            tumult_ingest::ManualImporter::new(&writer)
                .import_file(&csv_path, None)
                .unwrap();
        }
        let (ingest, writer_task) = IngestWriter::spawn(store.writer().unwrap(), 8);
        (db_path, store, ingest, writer_task)
    }

    fn span_count(store: &Store) -> u64 {
        let rows = store
            .read_only()
            .unwrap()
            .query_json_rows("SELECT count(*) AS c FROM spans")
            .unwrap();
        rows[0]["c"].as_u64().unwrap()
    }

    #[tokio::test]
    async fn run_lake_job_writes_parquet_and_the_committed_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let (db_path, _store, ingest, writer_task) = store_with_spans(
            dir.path(),
            "ts_ns,span_name,service_name\n123,resilience.experiment,demo\n",
        );
        let lake_dir = dir.path().join("lake");
        run_lake_job(
            &db_path,
            &ingest,
            &tumult_lake::lake::LakeConfig::new(lake_dir.clone(), 0),
        )
        .await
        .unwrap();

        let parquet = walk_for_parquet(&lake_dir);
        assert!(!parquet.is_empty(), "no parquet file written");
        assert!(
            lake_dir.join("_meta.json").exists(),
            "committed manifest written"
        );

        drop(ingest);
        writer_task.await.unwrap();
    }

    fn walk_for_parquet(dir: &std::path::Path) -> Vec<std::fs::DirEntry> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk_for_parquet(&path));
            } else if path.extension().is_some_and(|x| x == "parquet") {
                out.push(entry);
            }
        }
        out
    }

    #[tokio::test]
    async fn run_lake_job_refuses_retention_and_keeps_complete_history() {
        let dir = tempfile::tempdir().unwrap();
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as i64);
        let sixty_days_ago = now_ns - 60 * 86_400 * 1_000_000_000;
        let csv = format!(
            "ts_ns,span_name,service_name\n{sixty_days_ago},resilience.experiment,old\n{now_ns},resilience.experiment,fresh\n"
        );
        let (db_path, store, ingest, writer_task) = store_with_spans(dir.path(), &csv);
        assert_eq!(span_count(&store), 2);

        let error = run_lake_job(
            &db_path,
            &ingest,
            &tumult_lake::lake::LakeConfig::new(dir.path().join("lake"), 30),
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("historical"));
        // Historical reports retain their complete source evidence.
        assert_eq!(span_count(&store), 2);
        assert!(!dir.path().join("lake/_meta.json").exists());

        drop(ingest);
        writer_task.await.unwrap();
    }
}
