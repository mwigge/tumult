use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tumult_core::types::bounded_log_text;
use tumult_ingest::{Batch, IngestWriter};
use tumult_lake::Store;
use tumult_otel::logging::err;

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
                name: "lake.interval.invalid",
                { config.key = "KRONIKA_LAKE_INTERVAL", config.value = raw },
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
        name: "lake.export.completed",
        {
            rows = total,
            files = report.tables.iter().map(|t| t.files.len()).sum::<usize>(),
            file.path = %report.lake_dir,
        },
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
        name: "lake.export.enabled",
        {
            interval_s = interval.as_secs(),
            file.path = %cfg.dir.display(),
            retention_days = cfg.retention_days,
        },
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
                        tracing::warn!(
                            name: "lake.export.failed",
                            { error.message = %err(&e) },
                            "lake export job failed"
                        );
                    }
                }
                () = shutdown.cancelled() => {
                    tracing::info!(
                        name: "lake.export.stopping",
                        "lake export job stopping (shutdown)"
                    );
                    break;
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Backfill of the version rollup

/// How long one turn of the backfill may hold the writer.
pub(crate) const BACKFILL_TURN: Duration = Duration::from_millis(250);
/// The pause between two turns, in which telemetry is written.
pub(crate) const BACKFILL_PAUSE: Duration = Duration::from_secs(1);

/// Give the backfill one turn on the writer; `true` when the rollup is
/// complete.
async fn backfill_turn(ingest: &IngestWriter, budget: Duration) -> Result<bool> {
    let done = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&done);
    ingest
        .write(Batch::Exec(Box::new(move |writer| {
            let finished = writer
                .continue_version_backfill(budget)
                .map_err(|e| e.to_string())?;
            seen.store(finished, Ordering::Release);
            Ok(())
        })))
        .await?;
    Ok(done.load(Ordering::Acquire))
}

/// One turn of the job: `true` when the rollup is complete and the job
/// has nothing left to do. A turn that fails is logged and is not the
/// end.
async fn backfill_finished(ingest: &IngestWriter) -> bool {
    match backfill_turn(ingest, BACKFILL_TURN).await {
        Ok(true) => {
            tracing::info!("daemon.sightings.backfill.done");
            true
        }
        Ok(false) => false,
        Err(e) => {
            tracing::warn!(
                error.message = %bounded_log_text(&format!("{e:#}")),
                "daemon.sightings.backfill.failed"
            );
            false
        }
    }
}

/// Carry the backfill of the version rollup on after the open: a turn of
/// [`BACKFILL_TURN`] on the writer, then `pause` for telemetry, until the
/// rollup is complete. The open itself spends only a short budget, so a
/// large lake does not hold the daemon's start; until this job ends,
/// answers say `backfill: running`. A turn that fails is logged and tried
/// again. The task holds an `IngestWriter` clone and stops on `shutdown`.
pub(crate) fn spawn_version_backfill(
    ingest: IngestWriter,
    pause: Duration,
    shutdown: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            if backfill_finished(&ingest).await {
                break;
            }
            tokio::select! {
                () = tokio::time::sleep(pause) => {}
                () = shutdown.cancelled() => break,
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

    fn versioned_span(ts_ns: i64) -> tumult_lake::SpanRow {
        tumult_lake::SpanRow {
            ts_ns,
            trace_id: format!("t-{ts_ns}"),
            span_id: format!("s-{ts_ns}"),
            span_name: "GET /".into(),
            span_kind: "SERVER".into(),
            status_code: "OK".into(),
            service_name: "checkout".into(),
            service_version: Some("1.0.0".into()),
            target_environment: Some("demo".into()),
            events: "[]".into(),
            ..tumult_lake::SpanRow::default()
        }
    }

    /// A lake whose backfill the open left unfinished: spans on two days,
    /// an empty rollup, and the cursor at the start.
    fn lake_with_a_backfill_to_finish(dir: &std::path::Path) -> (PathBuf, Store) {
        const DAY_NS: i64 = 86_400_000_000_000;
        let db_path = dir.join("k.duckdb");
        let store = Store::open(&db_path).unwrap();
        let writer = store.writer().unwrap();
        writer
            .insert_spans(&[versioned_span(DAY_NS), versioned_span(3 * DAY_NS)])
            .unwrap();
        for sql in [
            "DELETE FROM service_version_sightings",
            "UPDATE sightings_backfill SET done = false, cursor_ns = 0",
        ] {
            writer.execute(sql, []).unwrap();
        }
        (db_path, store)
    }

    fn backfill_running(store: &Store) -> bool {
        store
            .read_only()
            .unwrap()
            .version_backfill_running()
            .unwrap()
    }

    #[tokio::test]
    async fn a_backfill_the_open_left_unfinished_is_carried_on_until_it_is_done() {
        let dir = tempfile::tempdir().unwrap();
        let (_db_path, store) = lake_with_a_backfill_to_finish(dir.path());
        assert!(backfill_running(&store));
        let (ingest, writer_task) = IngestWriter::spawn(store.writer().unwrap(), 8);

        let task = spawn_version_backfill(
            ingest.clone(),
            Duration::from_millis(5),
            tokio_util::sync::CancellationToken::new(),
        );
        tokio::time::timeout(Duration::from_secs(20), task)
            .await
            .expect("the backfill job did not end")
            .expect("the backfill job panicked");

        assert!(!backfill_running(&store));
        let seen = store.read_only().unwrap().version_sightings("checkout");
        assert_eq!(seen.unwrap().len(), 1);
        // The job gave its writer handle back: the drain ends.
        drop(ingest);
        tokio::time::timeout(Duration::from_secs(10), writer_task)
            .await
            .expect("ingest writer drain hung")
            .expect("ingest writer task panicked");
    }

    #[tokio::test]
    async fn a_backfill_job_stops_on_shutdown_and_when_the_writer_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let (_db_path, store) = lake_with_a_backfill_to_finish(dir.path());
        let (ingest, writer_task) = IngestWriter::spawn(store.writer().unwrap(), 8);
        writer_task.abort();
        let _ = writer_task.await;
        // Every turn fails now, and a failed turn does not end the job:
        // only the shutdown does.
        assert!(
            !backfill_finished(&ingest).await,
            "a failed turn is tried again"
        );
        let shutdown = tokio_util::sync::CancellationToken::new();
        let task = spawn_version_backfill(ingest, Duration::from_secs(3600), shutdown.clone());
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("the backfill job did not stop on shutdown")
            .expect("the backfill job panicked");
        assert!(backfill_running(&store));
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
