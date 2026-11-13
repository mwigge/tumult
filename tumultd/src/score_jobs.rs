//! DORA score jobs: the function register sync and the daily status
//! snapshot (`KRONIKA_SCORE_SNAPSHOT_INTERVAL`).
//!
//! At start the function register of the org file is written to the lake,
//! so the score reads functions, legal entities and stable register codes
//! from one place. Then, once per interval, the evidence status of every
//! requirement and function is stored for the day: the 180-day operating
//! rule counts those stored days.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tumult_compliance::dora::params::HouseParams;
use tumult_compliance::dora::{register, report};
use tumult_ingest::{Batch, IngestWriter};
use tumult_lake::{Store, Writer};

use crate::reports::parse_interval;

/// Environment variable setting the snapshot interval.
const INTERVAL_ENV: &str = "KRONIKA_SCORE_SNAPSHOT_INTERVAL";

/// The snapshot is a daily fact; a shorter interval only refreshes today's
/// rows more often.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(86_400);

/// `Some(24h)` by default; `KRONIKA_SCORE_SNAPSHOT_INTERVAL` overrides,
/// `0`/`off` disables the snapshot (the register is still synced).
pub(crate) fn snapshot_interval_from_env() -> Option<Duration> {
    let Ok(raw) = std::env::var(INTERVAL_ENV) else {
        return Some(DEFAULT_INTERVAL);
    };
    let raw = raw.trim();
    if raw.is_empty() || raw == "0" || raw.eq_ignore_ascii_case("off") {
        return None;
    }
    let Some(interval) = parse_interval(raw) else {
        tracing::warn!(
            value = raw,
            "invalid KRONIKA_SCORE_SNAPSHOT_INTERVAL (want e.g. 6h or 24h); using 24h"
        );
        return Some(DEFAULT_INTERVAL);
    };
    Some(interval)
}

/// What the score jobs need from the daemon's configuration.
#[derive(Clone)]
pub(crate) struct ScoreJobs {
    pub db_path: PathBuf,
    /// The org file holding the function register, when one is configured.
    pub org_file: Option<PathBuf>,
    pub manifests_dir: Option<PathBuf>,
    pub params: Arc<HouseParams>,
    /// `None` disables the daily snapshot.
    pub interval: Option<Duration>,
}

fn now_ns() -> i64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_nanos()).unwrap_or(i64::MAX)
}

/// Write the function register of `org_file` to the lake. `Ok(None)` when
/// there is no such file; otherwise the number of functions written.
async fn sync_register(org_file: &Path, ingest: &IngestWriter) -> Result<Option<usize>> {
    if !org_file.exists() {
        return Ok(None);
    }
    let declared = register::load(org_file)
        .map_err(|e| anyhow::anyhow!("function register {}: {e}", org_file.display()))?;
    let functions = declared.ict.functions.len();
    let at = now_ns();
    ingest
        .write(Batch::Exec(Box::new(move |writer: &Writer| {
            writer
                .sync_ict_register(&declared.ict, at)
                .map_err(|e| e.to_string())
        })))
        .await
        .context("write the function register")?;
    Ok(Some(functions))
}

/// Compute today's status rows and store them. Returns the row count.
async fn run_snapshot(jobs: &ScoreJobs, ingest: &IngestWriter, as_of_ns: i64) -> Result<usize> {
    let (db, params, manifests) = (
        jobs.db_path.clone(),
        Arc::clone(&jobs.params),
        jobs.manifests_dir.clone(),
    );
    let rows = tokio::task::spawn_blocking(move || -> Result<_> {
        let workloads = tumult_api::handlers::dora::manifest_workloads(manifests.as_deref())
            .map_err(anyhow::Error::msg)?;
        let reader = Store::at(&db).read_only().context("open store read-only")?;
        report::snapshot_all(&reader, &params, &workloads, as_of_ns)
            .map_err(|e| anyhow::anyhow!("score the lake: {e}"))
    })
    .await??;
    let count = rows.len();
    ingest
        .write(Batch::Exec(Box::new(move |writer: &Writer| {
            writer
                .upsert_requirement_status(&rows, as_of_ns)
                .map_err(|e| e.to_string())
        })))
        .await
        .context("write the status snapshot")?;
    Ok(count)
}

async fn sync_register_logged(jobs: &ScoreJobs, ingest: &IngestWriter) {
    let Some(org_file) = jobs.org_file.as_deref() else {
        return;
    };
    match sync_register(org_file, ingest).await {
        Ok(Some(functions)) => {
            tracing::info!(functions, path = %org_file.display(), "function register synced");
        }
        Ok(None) => {
            tracing::info!(path = %org_file.display(), "no org file; function register not synced");
        }
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "function register sync failed; the lake keeps its previous register");
        }
    }
}

async fn run_snapshot_logged(jobs: &ScoreJobs, ingest: &IngestWriter) {
    match run_snapshot(jobs, ingest, now_ns()).await {
        Ok(rows) => tracing::info!(rows, "requirement status snapshot stored"),
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "requirement status snapshot failed");
        }
    }
}

/// Spawn the score jobs: the register sync once, then one status snapshot
/// immediately and one per interval. Failures are logged; the snapshot is
/// an upsert per day, so a retry is idempotent. The task holds an
/// `IngestWriter` clone, so it stops on `shutdown` and the returned handle
/// is awaited before the daemon drains the writer channel.
pub(crate) fn spawn_score_jobs(
    jobs: ScoreJobs,
    ingest: IngestWriter,
    shutdown: tokio_util::sync::CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tracing::info!(
        interval = ?jobs.interval,
        org_file = ?jobs.org_file,
        "score jobs enabled"
    );
    tokio::spawn(async move {
        sync_register_logged(&jobs, &ingest).await;
        let Some(interval) = jobs.interval else {
            tracing::info!("requirement status snapshot disabled");
            return;
        };
        let mut ticker = tokio::time::interval(interval);
        loop {
            tokio::select! {
                _ = ticker.tick() => run_snapshot_logged(&jobs, &ingest).await,
                () = shutdown.cancelled() => {
                    tracing::info!("score jobs stopping (shutdown)");
                    break;
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORG: &str = "
legal_entities:
  - {lei: 529900T8BM49AURSDO55, name: Demo Firm, profile: simplified}
functions:
  - {id: cif-0001, name: Payments, tier: critical, services: [payments-api]}
  - {id: cif-0002, name: Reporting, tier: important}
";

    struct Daemon {
        _dir: tempfile::TempDir,
        store: Store,
        jobs: ScoreJobs,
        ingest: IngestWriter,
        writer_task: tokio::task::JoinHandle<()>,
    }

    fn daemon(org: Option<&str>, interval: Option<Duration>) -> Daemon {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("k.duckdb");
        let store = Store::open(&db_path).unwrap();
        let org_file = dir.path().join("org.yaml");
        if let Some(text) = org {
            std::fs::write(&org_file, text).unwrap();
        }
        let (ingest, writer_task) = IngestWriter::spawn(store.writer().unwrap(), 8);
        Daemon {
            _dir: dir,
            store,
            jobs: ScoreJobs {
                db_path,
                org_file: Some(org_file),
                manifests_dir: None,
                params: Arc::new(HouseParams::default()),
                interval,
            },
            ingest,
            writer_task,
        }
    }

    fn count(store: &Store, table: &str) -> u64 {
        let rows = store
            .read_only()
            .unwrap()
            .query_json_rows(&format!("SELECT count(*) AS c FROM {table}"))
            .unwrap();
        rows[0]["c"].as_u64().unwrap()
    }

    async fn wait(task: tokio::task::JoinHandle<()>, what: &str) {
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap_or_else(|_| panic!("{what} did not finish"))
            .unwrap_or_else(|e| panic!("{what} panicked: {e}"));
    }

    #[tokio::test]
    async fn the_register_of_the_org_file_is_written_to_the_lake() {
        let d = daemon(Some(ORG), None);
        let org_file = d.jobs.org_file.clone().unwrap();
        let synced = sync_register(&org_file, &d.ingest).await.unwrap();
        assert_eq!(synced, Some(2));
        assert_eq!(count(&d.store, "ict_functions"), 2);
        assert_eq!(count(&d.store, "ict_legal_entities"), 1);
        assert_eq!(count(&d.store, "ict_function_services"), 1);
        // A second start keeps the register codes: same rows, not new ones.
        sync_register(&org_file, &d.ingest).await.unwrap();
        assert_eq!(count(&d.store, "ict_functions"), 2);
        drop(d.ingest);
        wait(d.writer_task, "writer").await;
    }

    #[tokio::test]
    async fn a_missing_org_file_is_not_an_error_and_an_invalid_one_is() {
        let d = daemon(None, None);
        let org_file = d.jobs.org_file.clone().unwrap();
        assert_eq!(sync_register(&org_file, &d.ingest).await.unwrap(), None);
        std::fs::write(
            &org_file,
            "functions:\n  - {id: cif-0001, name: A, tier: critical}\n  - {id: cif-0001, name: B, tier: critical}\n",
        )
        .unwrap();
        let err = sync_register(&org_file, &d.ingest).await.unwrap_err();
        assert!(format!("{err:#}").contains("cif-0001"), "{err:#}");
        assert_eq!(count(&d.store, "ict_functions"), 0);
        drop(d.ingest);
        wait(d.writer_task, "writer").await;
    }

    #[tokio::test]
    async fn the_snapshot_stores_one_day_of_status_rows_and_is_idempotent() {
        let d = daemon(Some(ORG), None);
        let org_file = d.jobs.org_file.clone().unwrap();
        sync_register(&org_file, &d.ingest).await.unwrap();
        let at = now_ns();
        let rows = run_snapshot(&d.jobs, &d.ingest, at).await.unwrap();
        assert!(rows > 0);
        assert_eq!(count(&d.store, "requirement_status_daily"), rows as u64);
        // The same day again replaces the rows instead of adding to them.
        let again = run_snapshot(&d.jobs, &d.ingest, at + 1).await.unwrap();
        assert_eq!(again, rows);
        assert_eq!(count(&d.store, "requirement_status_daily"), rows as u64);
        drop(d.ingest);
        wait(d.writer_task, "writer").await;
    }

    /// The spawned task syncs the register, snapshots at once (not after
    /// the first interval), and stops on shutdown so the writer can drain.
    #[tokio::test]
    async fn the_spawned_jobs_sync_snapshot_and_stop_on_shutdown() {
        let d = daemon(Some(ORG), Some(Duration::from_secs(3600)));
        let shutdown = tokio_util::sync::CancellationToken::new();
        let task = spawn_score_jobs(d.jobs.clone(), d.ingest.clone(), shutdown.clone());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while count(&d.store, "requirement_status_daily") == 0 {
            assert!(std::time::Instant::now() < deadline, "no snapshot stored");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(count(&d.store, "ict_functions"), 2);
        shutdown.cancel();
        wait(task, "score jobs").await;
        drop(d.ingest);
        wait(d.writer_task, "writer").await;
    }

    #[tokio::test]
    async fn a_disabled_snapshot_still_syncs_the_register_and_then_ends() {
        let d = daemon(Some(ORG), None);
        let shutdown = tokio_util::sync::CancellationToken::new();
        let task = spawn_score_jobs(d.jobs.clone(), d.ingest.clone(), shutdown);
        wait(task, "score jobs").await;
        assert_eq!(count(&d.store, "ict_functions"), 2);
        assert_eq!(count(&d.store, "requirement_status_daily"), 0);
        drop(d.ingest);
        wait(d.writer_task, "writer").await;
    }

    #[test]
    fn the_snapshot_interval_defaults_to_daily_and_honours_overrides() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(INTERVAL_ENV);
        assert_eq!(snapshot_interval_from_env(), Some(DEFAULT_INTERVAL));
        for off in ["", "0", "off", "OFF"] {
            std::env::set_var(INTERVAL_ENV, off);
            assert_eq!(snapshot_interval_from_env(), None, "{off:?} must disable");
        }
        std::env::set_var(INTERVAL_ENV, "6h");
        assert_eq!(
            snapshot_interval_from_env(),
            Some(Duration::from_secs(21_600))
        );
        std::env::set_var(INTERVAL_ENV, "bogus");
        assert_eq!(snapshot_interval_from_env(), Some(DEFAULT_INTERVAL));
        std::env::remove_var(INTERVAL_ENV);
    }
}
