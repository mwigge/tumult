//! DORA score jobs: the function register reload
//! (`KRONIKA_REGISTER_RELOAD_INTERVAL`) and the daily status snapshot
//! (`KRONIKA_SCORE_SNAPSHOT_INTERVAL`).
//!
//! At start and then once per reload interval the org file is read
//! again. A valid file that the lake does not hold yet is written to it,
//! so the score reads functions, legal entities and stable register
//! codes from one place; an invalid file leaves the lake as it is. The
//! score endpoints report which of the two is the case
//! (`register_state`).
//!
//! Once per snapshot interval, and as soon as a new UTC day is noticed,
//! the evidence status of every requirement and function, and the level
//! of every legal entity, are stored for the day. The 180-day operating
//! rule and the maturity history read those stored days and nothing
//! else: a day on which no snapshot was stored (the daemon was down, or
//! the snapshot is disabled) is not an operating day, and no later run
//! fills it in.
//!
//! A day is not stored while the default parameters stand in for an org
//! file that is refused and never was valid: that day would be scored
//! with parameters the house did not choose. The skip is logged, counted
//! and shown in every score response (`snapshot`).
//!
//! Nor is a day stored while proof days are in use
//! (`KRONIKA_SCORE_PROOF_DAYS`): it would be scored on days the proof
//! tool wrote and would stay in the lake as a real day afterwards.

use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;
use tumult_api::score::ScoreRuntime;
use tumult_compliance::dora::report;
use tumult_core::compliance::truncated;
use tumult_ingest::{Batch, IngestWriter};
use tumult_lake::{Store, Writer};

use crate::reports::parse_interval;

/// Environment variable setting the snapshot interval.
const INTERVAL_ENV: &str = "KRONIKA_SCORE_SNAPSHOT_INTERVAL";
/// Environment variable setting how often the org file is read again.
const RELOAD_ENV: &str = "KRONIKA_REGISTER_RELOAD_INTERVAL";

/// The snapshot is a daily fact; a shorter interval only refreshes today's
/// rows more often.
const DEFAULT_INTERVAL: Duration = Duration::from_secs(86_400);
/// A changed org file is in the lake within this time.
const DEFAULT_RELOAD: Duration = Duration::from_secs(60);
/// A new UTC day is noticed, and stored, within this time.
pub(crate) const DAY_CHECK: Duration = Duration::from_secs(60);
/// Most bytes of an error or a configured value in one log event.
const LOG_VALUE_MAX_LEN: usize = 512;

/// The interval of `key`: `default` when unset or not an interval, `None`
/// when empty, `0` or `off`.
fn interval_from_env(key: &'static str, default: Duration) -> Option<Duration> {
    let Ok(raw) = std::env::var(key) else {
        return Some(default);
    };
    let raw = raw.trim();
    if raw.is_empty() || raw == "0" || raw.eq_ignore_ascii_case("off") {
        return None;
    }
    let Some(interval) = parse_interval(raw) else {
        tracing::warn!(
            name: "score.interval.invalid",
            {
                config.key = key,
                config.value = ?truncated(raw, LOG_VALUE_MAX_LEN),
                config.default_s = default.as_secs(),
            },
            "invalid interval (want e.g. 60s, 6h or 24h); using the default"
        );
        return Some(default);
    };
    Some(interval)
}

/// `Some(24h)` by default; `KRONIKA_SCORE_SNAPSHOT_INTERVAL` overrides,
/// `0`/`off` disables the snapshot (the register is still synced).
pub(crate) fn snapshot_interval_from_env() -> Option<Duration> {
    interval_from_env(INTERVAL_ENV, DEFAULT_INTERVAL)
}

/// `Some(60s)` by default; `KRONIKA_REGISTER_RELOAD_INTERVAL` overrides,
/// `0`/`off` reads the org file at start only.
pub(crate) fn reload_interval_from_env() -> Option<Duration> {
    interval_from_env(RELOAD_ENV, DEFAULT_RELOAD)
}

/// What the score jobs need from the daemon's configuration.
#[derive(Clone)]
pub(crate) struct ScoreJobs {
    pub db_path: PathBuf,
    /// The org file, the manifests and the house parameters, shared with
    /// the score endpoints.
    pub runtime: Arc<ScoreRuntime>,
    /// `None` disables the daily snapshot.
    pub interval: Option<Duration>,
    /// `None` reads the org file and the manifests at start only.
    pub reload: Option<Duration>,
    /// How often the job looks whether a new UTC day has begun.
    pub day_check: Duration,
    /// The wall clock, in nanoseconds since the Unix epoch.
    pub now: fn() -> i64,
}

/// What became of one attempt to store the day.
#[derive(Debug, PartialEq, Eq)]
enum Attempt {
    /// The day is stored, in this many status rows.
    Stored(usize),
    /// Nothing was stored; `total` attempts were skipped since start.
    Skipped { total: u64 },
    /// Nothing was stored: proof days are in use.
    ProofDays,
}

/// Store the day of `as_of_ns`, unless the score runtime says a day
/// scored now must not be stored.
async fn snapshot_unless_blocked(
    jobs: &ScoreJobs,
    ingest: &IngestWriter,
    as_of_ns: i64,
) -> Result<Attempt> {
    if jobs.runtime.snapshot_block().is_some() {
        let total = jobs.runtime.snapshot_skipped();
        return Ok(Attempt::Skipped { total });
    }
    if jobs.runtime.proof_days() {
        return Ok(Attempt::ProofDays);
    }
    run_snapshot(jobs, ingest, as_of_ns)
        .await
        .map(Attempt::Stored)
}

pub(crate) fn now_ns() -> i64 {
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_nanos()).unwrap_or(i64::MAX)
}

/// Read the org file and the manifests again and, when the org file is
/// valid and not in the lake, write its register there. `Ok(None)` when
/// there was nothing to write; otherwise the number of functions written.
async fn sync_register(jobs: &ScoreJobs, ingest: &IngestWriter) -> Result<Option<usize>> {
    let runtime = Arc::clone(&jobs.runtime);
    let pending = tokio::task::spawn_blocking(move || {
        runtime.refresh_workloads();
        runtime.check(now_ns())
    })
    .await
    .context("read the org file")?;
    let Some(register) = pending else {
        return Ok(None);
    };
    let functions = register.functions.len();
    let at = now_ns();
    let written = ingest
        .write(Batch::Exec(Box::new(move |writer: &Writer| {
            writer
                .sync_ict_register(&register, at)
                .map_err(|e| e.to_string())
        })))
        .await;
    if let Err(e) = written {
        jobs.runtime.sync_failed(&e.to_string());
        return Err(e).context("write the function register");
    }
    jobs.runtime.synced(now_ns());
    Ok(Some(functions))
}

/// Compute the status rows and the levels of the day of `as_of_ns` and
/// store them. Returns the number of status rows.
async fn run_snapshot(jobs: &ScoreJobs, ingest: &IngestWriter, as_of_ns: i64) -> Result<usize> {
    let (db, runtime) = (jobs.db_path.clone(), Arc::clone(&jobs.runtime));
    let snapshot = tokio::task::spawn_blocking(move || -> Result<_> {
        let workloads = runtime.workloads().map_err(anyhow::Error::msg)?;
        let reader = Store::at(&db).read_only().context("open store read-only")?;
        // Never on proof days: what is stored here is real history.
        report::snapshot_all(&reader, &runtime.params(), &workloads, as_of_ns, false)
            .map_err(|e| anyhow::anyhow!("score the lake: {e}"))
    })
    .await??;
    let count = snapshot.status.len();
    ingest
        .write(Batch::Exec(Box::new(move |writer: &Writer| {
            snapshot
                .store(writer, as_of_ns)
                .map(|_| ())
                .map_err(|e| e.to_string())
        })))
        .await
        .context("write the status snapshot")?;
    // The stored days changed: no response built before this is reused.
    jobs.runtime.invalidate();
    Ok(count)
}

fn error_text(e: &anyhow::Error) -> String {
    format!("{e:#}")
}

async fn sync_register_logged(jobs: &ScoreJobs, ingest: &IngestWriter) {
    match sync_register(jobs, ingest).await {
        Ok(Some(functions)) => {
            tracing::info!(
                name: "score.register.written",
                { resilience.register.functions = functions },
                "function register written to the lake"
            );
        }
        Ok(None) => {}
        Err(e) => {
            tracing::warn!(
                name: "score.register.write_failed",
                {
                    error.type = "register_sync_failed",
                    error.message = ?truncated(&error_text(&e), LOG_VALUE_MAX_LEN),
                },
                "function register not written; the lake keeps its previous register"
            );
        }
    }
}

/// Try to store the day of `as_of_ns`. `false` when the attempt failed
/// and the day is still to be stored.
async fn run_snapshot_logged(jobs: &ScoreJobs, ingest: &IngestWriter, as_of_ns: i64) -> bool {
    match snapshot_unless_blocked(jobs, ingest, as_of_ns).await {
        Ok(Attempt::Stored(rows)) => tracing::info!(
            name: "score.snapshot.stored",
            { resilience.snapshot.rows = rows },
            "requirement status snapshot stored"
        ),
        Ok(Attempt::ProofDays) => tracing::warn!(
            name: "score.snapshot.proof_days",
            { error.type = "status_snapshot_proof_days" },
            "requirement status snapshot not stored: proof days are in use"
        ),
        Ok(Attempt::Skipped { total }) => tracing::warn!(
            name: "score.snapshot.skipped",
            {
                error.type = "status_snapshot_skipped",
                error.message = ?jobs
                    .runtime
                    .snapshot_block()
                    .as_deref()
                    .map(|reason| truncated(reason, LOG_VALUE_MAX_LEN)),
                resilience.snapshot.skipped_total = total,
            },
            "requirement status snapshot not stored: the org file is refused and no valid one was read"
        ),
        Err(e) => {
            tracing::warn!(
                name: "score.snapshot.failed",
                {
                    error.type = "status_snapshot_failed",
                    error.message = ?truncated(&error_text(&e), LOG_VALUE_MAX_LEN),
                },
                "requirement status snapshot failed"
            );
            return false;
        }
    }
    true
}

/// Days since the Unix epoch (UTC) of a wall-clock reading.
fn utc_day(now_ns: i64) -> i64 {
    now_ns.div_euclid(86_400 * 1_000_000_000)
}

/// `job` to its end, or `None` as soon as `shutdown` is cancelled. A job
/// given up here is dropped where it stands; the snapshot is one upsert
/// per day, so the next start stores the day again.
async fn until_cancelled<T>(
    job: impl Future<Output = T>,
    shutdown: &CancellationToken,
) -> Option<T> {
    tokio::select! {
        biased;
        () = shutdown.cancelled() => None,
        done = job => Some(done),
    }
}

/// One attempt at the day the clock shows. An attempt that ends, stored
/// or skipped, is remembered in `tried_day`; a failed one is not, so the
/// day check tries the day again. `None` when shutdown cut it short.
async fn store_day(
    jobs: &ScoreJobs,
    ingest: &IngestWriter,
    shutdown: &CancellationToken,
    tried_day: &mut Option<i64>,
) -> Option<()> {
    let now = (jobs.now)();
    let ended = until_cancelled(run_snapshot_logged(jobs, ingest, now), shutdown).await?;
    if ended {
        *tried_day = Some(utc_day(now));
    }
    Some(())
}

/// The next tick of `ticker`; never, when there is none.
async fn tick(ticker: &mut Option<tokio::time::Interval>) {
    match ticker {
        Some(ticker) => {
            ticker.tick().await;
        }
        None => std::future::pending().await,
    }
}

/// Spawn the score jobs: the register sync at once and on every reload
/// interval, one status snapshot at once, one per snapshot interval and
/// one as soon as a UTC day begins that has not been tried (the interval
/// counts from the start of the daemon, so alone it can step over a
/// day). Failures are logged and the day is tried again on the next day
/// check; the snapshot is an upsert per day, so a retry is idempotent.
/// The task holds an `IngestWriter` clone, so it stops on
/// `shutdown`, giving up a running snapshot, and the returned handle is
/// awaited before the daemon drains the writer channel.
pub(crate) fn spawn_score_jobs(
    jobs: ScoreJobs,
    ingest: IngestWriter,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tracing::info!(
        name: "score.jobs.enabled",
        {
            score.snapshot.interval_s = ?jobs.interval.map(|d| d.as_secs()),
            score.reload.interval_s = ?jobs.reload.map(|d| d.as_secs()),
            file.path = ?jobs.runtime.org_file(),
        },
        "score jobs enabled"
    );
    tokio::spawn(async move {
        if until_cancelled(sync_register_logged(&jobs, &ingest), &shutdown)
            .await
            .is_none()
        {
            return;
        }
        if jobs.interval.is_none() {
            tracing::info!(
                name: "score.snapshot.disabled",
                "requirement status snapshot disabled"
            );
        }
        if jobs.interval.is_none() && jobs.reload.is_none() {
            return;
        }
        let mut snapshot = jobs.interval.map(tokio::time::interval);
        let mut day_watch = jobs.interval.map(|_| {
            tokio::time::interval_at(tokio::time::Instant::now() + jobs.day_check, jobs.day_check)
        });
        // The last UTC day an attempt ended for, stored or skipped.
        let mut tried_day: Option<i64> = None;
        let mut reload = jobs
            .reload
            .map(|every| tokio::time::interval_at(tokio::time::Instant::now() + every, every));
        loop {
            let finished = tokio::select! {
                () = tick(&mut snapshot) => {
                    store_day(&jobs, &ingest, &shutdown, &mut tried_day).await
                }
                () = tick(&mut day_watch) => {
                    if tried_day == Some(utc_day((jobs.now)())) {
                        continue;
                    }
                    store_day(&jobs, &ingest, &shutdown, &mut tried_day).await
                }
                () = tick(&mut reload) => {
                    until_cancelled(sync_register_logged(&jobs, &ingest), &shutdown).await
                }
                () = shutdown.cancelled() => None,
            };
            if finished.is_none() {
                tracing::info!(name: "score.jobs.stopping", "score jobs stopping (shutdown)");
                break;
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use tumult_api::score::RegisterState;
    use tumult_compliance::dora::params::HouseParams;

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
        org_file: PathBuf,
        jobs: ScoreJobs,
        ingest: IngestWriter,
        writer_task: tokio::task::JoinHandle<()>,
    }

    impl Daemon {
        fn params(&self) -> Arc<HouseParams> {
            self.jobs.runtime.params()
        }

        fn register_state(&self) -> RegisterState {
            self.jobs.runtime.register_state()
        }

        async fn stop(self) {
            drop(self.ingest);
            wait(self.writer_task, "writer").await;
        }
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
        let runtime = ScoreRuntime::open(
            Some(org_file.clone()),
            None,
            tumult_api::score::DEFAULT_CACHE_TTL,
            now_ns(),
        );
        Daemon {
            _dir: dir,
            store,
            org_file,
            jobs: ScoreJobs {
                db_path,
                runtime: Arc::new(runtime),
                interval,
                reload: None,
                day_check: DAY_CHECK,
                now: now_ns,
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

    /// Poll until `done` holds; panic with `what` after ten seconds.
    async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(std::time::Instant::now() < deadline, "{what}");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[tokio::test]
    async fn the_register_of_the_org_file_is_written_to_the_lake() {
        let d = daemon(Some(ORG), None);
        assert_eq!(d.register_state(), RegisterState::Stale);
        let synced = sync_register(&d.jobs, &d.ingest).await.unwrap();
        assert_eq!(synced, Some(2));
        assert_eq!(d.register_state(), RegisterState::Ok);
        assert_eq!(count(&d.store, "ict_functions"), 2);
        assert_eq!(count(&d.store, "ict_legal_entities"), 1);
        assert_eq!(count(&d.store, "ict_function_services"), 1);
        // An unchanged file is not written again.
        assert_eq!(sync_register(&d.jobs, &d.ingest).await.unwrap(), None);
        assert_eq!(count(&d.store, "ict_functions"), 2);
        d.stop().await;
    }

    #[tokio::test]
    async fn a_missing_org_file_is_no_register_and_an_invalid_one_leaves_the_lake_alone() {
        let d = daemon(None, None);
        assert_eq!(sync_register(&d.jobs, &d.ingest).await.unwrap(), None);
        assert_eq!(d.register_state(), RegisterState::None);

        std::fs::write(&d.org_file, ORG).unwrap();
        assert_eq!(sync_register(&d.jobs, &d.ingest).await.unwrap(), Some(2));
        std::fs::write(
            &d.org_file,
            "functions:\n  - {id: cif-0001, name: A, tier: critical}\n  - {id: cif-0001, name: B, tier: critical}\n",
        )
        .unwrap();
        assert_eq!(sync_register(&d.jobs, &d.ingest).await.unwrap(), None);
        assert_eq!(d.register_state(), RegisterState::Invalid);
        assert_eq!(
            count(&d.store, "ict_functions"),
            2,
            "the lake keeps its register"
        );
        d.stop().await;
    }

    /// The org file is read again on the reload interval: a change is in
    /// the lake without a restart, and a broken file is reported and
    /// changes nothing.
    #[tokio::test]
    async fn a_changed_org_file_is_written_on_the_next_reload() {
        let mut d = daemon(Some(ORG), None);
        d.jobs.reload = Some(Duration::from_millis(40));
        let shutdown = CancellationToken::new();
        let task = spawn_score_jobs(d.jobs.clone(), d.ingest.clone(), shutdown.clone());
        eventually("the register is not written at start", || {
            d.register_state() == RegisterState::Ok
        })
        .await;
        assert_eq!(count(&d.store, "ict_functions"), 2);

        let three = format!("{ORG}  - {{id: cif-0003, name: Custody, tier: critical}}\n");
        std::fs::write(&d.org_file, three).unwrap();
        eventually("the changed register is not written", || {
            count(&d.store, "ict_functions") == 3
        })
        .await;
        eventually("the written register is not reported", || {
            d.register_state() == RegisterState::Ok
        })
        .await;

        std::fs::write(&d.org_file, "functions: [{id: NOT-AN-ID}]\n").unwrap();
        eventually("the broken file is not reported", || {
            d.register_state() == RegisterState::Invalid
        })
        .await;
        assert_eq!(count(&d.store, "ict_functions"), 3);

        shutdown.cancel();
        wait(task, "score jobs").await;
        d.stop().await;
    }

    const DAY_NS: i64 = 86_400 * 1_000_000_000;
    /// 2027-01-15 08:00 UTC.
    const T0: i64 = 1_800_000_000 * 1_000_000_000;

    /// A passed restore test of the Payments service at `ts`, written the
    /// way ingest writes it.
    async fn seed_restore_run(d: &Daemon, ts: i64) {
        let pairs = |list: &[(&str, &str)]| -> Vec<(String, String)> {
            list.iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect()
        };
        let span = tumult_lake::SpanRow {
            ts_ns: ts,
            trace_id: "t-restore".into(),
            span_id: "s-restore".into(),
            span_name: "resilience.experiment".into(),
            span_kind: "Internal".into(),
            service_name: "tumult".into(),
            experiment_id: Some("restore-1".into()),
            experiment_name: Some("payments-api restore".into()),
            target_system: Some("payments-api".into()),
            target_environment: Some("prod".into()),
            span_attrs: pairs(&[("resilience.test.kind", "dr")]),
            events: "[]".into(),
            ..tumult_lake::SpanRow::default()
        };
        let log = tumult_lake::LogRow {
            ts_ns: ts,
            severity_text: "INFO".into(),
            body: "experiment.completed".into(),
            service_name: "tumult".into(),
            log_attrs: pairs(&[("experiment_id", "restore-1"), ("status", "completed")]),
            ..tumult_lake::LogRow::default()
        };
        d.ingest.write(Batch::Spans(vec![span])).await.unwrap();
        d.ingest.write(Batch::Logs(vec![log])).await.unwrap();
    }

    /// Status and streak of the yearly restore-test row for Payments.
    fn restore_status(d: &Daemon, as_of_ns: i64) -> (&'static str, u32) {
        use tumult_compliance::dora::coverage::Matrix;
        use tumult_compliance::dora::{loader, status};
        let request = loader::Request {
            envs: &[],
            lei: None,
            profile: None,
            as_of_ns,
            workloads: &[],
            proof_days: false,
        };
        let params = d.params();
        let reader = d.store.read_only().unwrap();
        let loaded = loader::load(&reader, request, &params).unwrap();
        let matrix = Matrix::build(&loaded.inputs, &params);
        let row = matrix.row("rts1774.art40").expect("the restore-test row");
        let payments = loaded
            .inputs
            .functions
            .iter()
            .position(|f| f.id == "cif-0001")
            .unwrap();
        let cell = row.cells.iter().find(|c| c.function == payments).unwrap();
        (
            status::cell_status(cell, &params).as_str(),
            cell.streak_days,
        )
    }

    /// The Operating streak is the days the job stored, not the age of
    /// the evidence: a run 200 days old with no stored day is Existing,
    /// a day the job missed breaks the streak, and storing that day
    /// later the same way (here: the job with that day's clock) is what
    /// closes it.
    #[tokio::test]
    async fn operating_counts_only_the_days_the_job_stored() {
        let d = daemon(Some(ORG), None);
        sync_register(&d.jobs, &d.ingest).await.unwrap();
        seed_restore_run(&d, T0).await;
        let at = |day: i64| T0 + day * DAY_NS;

        assert_eq!(restore_status(&d, at(200)), ("existing", 0));

        for day in (20..200).filter(|day| *day != 150) {
            run_snapshot(&d.jobs, &d.ingest, at(day)).await.unwrap();
        }
        assert_eq!(restore_status(&d, at(200)), ("existing", 49));

        run_snapshot(&d.jobs, &d.ingest, at(150)).await.unwrap();
        assert_eq!(restore_status(&d, at(200)), ("operating", 180));
        // One day later, before that day's snapshot: yesterday is missing.
        assert_eq!(restore_status(&d, at(201)), ("existing", 0));
        d.stop().await;
    }

    /// One snapshot stores the status rows of the day and the level of
    /// each legal entity, replaces them when run again the same day, and
    /// tells the score endpoints that what they cached is out of date.
    #[tokio::test]
    async fn the_snapshot_stores_one_day_of_status_and_level_and_is_idempotent() {
        let d = daemon(Some(ORG), None);
        sync_register(&d.jobs, &d.ingest).await.unwrap();
        let at = now_ns();
        let before = d.jobs.runtime.generation();
        let rows = run_snapshot(&d.jobs, &d.ingest, at).await.unwrap();
        assert!(rows > 0);
        assert!(
            d.jobs.runtime.generation() > before,
            "the cache is not invalidated"
        );
        assert_eq!(count(&d.store, "requirement_status_daily"), rows as u64);
        assert_eq!(count(&d.store, "maturity_daily"), 1);
        // The same day again replaces the rows instead of adding to them.
        let again = run_snapshot(&d.jobs, &d.ingest, at + 1).await.unwrap();
        assert_eq!(again, rows);
        assert_eq!(count(&d.store, "requirement_status_daily"), rows as u64);
        assert_eq!(count(&d.store, "maturity_daily"), 1);
        d.stop().await;
    }

    /// Defaults standing in for a file that was never valid are not the
    /// house's parameters: the day is not stored, and the skip is
    /// counted. Once a valid file was read, a later broken one keeps
    /// its parameters and the days are stored again.
    #[tokio::test]
    async fn no_day_is_stored_while_defaults_stand_in_for_a_refused_file() {
        const BROKEN: &str = "functions: [{id: NOT-AN-ID}]\n";
        let d = daemon(Some(BROKEN), None);
        assert_eq!(sync_register(&d.jobs, &d.ingest).await.unwrap(), None);
        for total in 1..=2 {
            let attempt = snapshot_unless_blocked(&d.jobs, &d.ingest, now_ns())
                .await
                .unwrap();
            assert_eq!(attempt, Attempt::Skipped { total });
        }
        assert_eq!(count(&d.store, "requirement_status_daily"), 0);
        assert_eq!(count(&d.store, "maturity_daily"), 0);

        std::fs::write(&d.org_file, ORG).unwrap();
        sync_register(&d.jobs, &d.ingest).await.unwrap();
        let attempt = snapshot_unless_blocked(&d.jobs, &d.ingest, now_ns())
            .await
            .unwrap();
        assert!(
            matches!(attempt, Attempt::Stored(rows) if rows > 0),
            "{attempt:?}"
        );
        assert_eq!(count(&d.store, "maturity_daily"), 1);

        std::fs::write(&d.org_file, BROKEN).unwrap();
        sync_register(&d.jobs, &d.ingest).await.unwrap();
        assert_eq!(d.register_state(), RegisterState::Invalid);
        let attempt = snapshot_unless_blocked(&d.jobs, &d.ingest, now_ns() + DAY_NS)
            .await
            .unwrap();
        assert!(matches!(attempt, Attempt::Stored(_)), "{attempt:?}");
        assert_eq!(count(&d.store, "maturity_daily"), 2);
        d.stop().await;
    }

    /// With the proof opt-in the day would be scored on the days the
    /// proof tool wrote. Such a day is not stored: it would stay in the
    /// lake as a real one after the opt-in is taken away.
    #[tokio::test]
    async fn no_day_is_stored_while_proof_days_are_in_use() {
        let mut d = daemon(Some(ORG), None);
        let runtime = ScoreRuntime::open(
            Some(d.org_file.clone()),
            None,
            tumult_api::score::DEFAULT_CACHE_TTL,
            now_ns(),
        );
        d.jobs.runtime = Arc::new(runtime.with_proof_days(true));
        sync_register(&d.jobs, &d.ingest).await.unwrap();
        let attempt = snapshot_unless_blocked(&d.jobs, &d.ingest, now_ns())
            .await
            .unwrap();
        assert_eq!(attempt, Attempt::ProofDays);
        assert_eq!(count(&d.store, "requirement_status_daily"), 0);
        assert_eq!(count(&d.store, "maturity_daily"), 0);
        assert_eq!(
            d.jobs.runtime.snapshot_skipped(),
            1,
            "not counted as a refused org file"
        );
        // The attempt ends: the day check does not try the day again.
        assert!(run_snapshot_logged(&d.jobs, &d.ingest, now_ns()).await);
        assert_eq!(count(&d.store, "maturity_daily"), 0);
        d.stop().await;
    }

    static CLOCK: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(T0);

    /// How often the job has read the clock: once per attempt and once
    /// per day check.
    static CLOCK_READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn test_clock() -> i64 {
        CLOCK_READS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        CLOCK.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Wait until the job has read the clock `more` times from now on.
    async fn clock_reads(more: u64) {
        let reads = || CLOCK_READS.load(std::sync::atomic::Ordering::SeqCst);
        let from = reads();
        eventually("the job stopped checking the day", || {
            reads() >= from + more
        })
        .await;
    }

    /// The snapshot follows the UTC day, not the time since the last
    /// one: a day that begins is stored when it is noticed, long before
    /// the interval has passed, and stored once.
    #[tokio::test]
    async fn a_new_utc_day_is_stored_without_waiting_for_the_interval() {
        let mut d = daemon(Some(ORG), Some(Duration::from_secs(86_400)));
        d.jobs.day_check = Duration::from_millis(20);
        d.jobs.now = test_clock;
        let shutdown = CancellationToken::new();
        let task = spawn_score_jobs(d.jobs.clone(), d.ingest.clone(), shutdown.clone());
        eventually("the first day is not stored", || {
            count(&d.store, "maturity_daily") == 1
        })
        .await;
        // 23:59:59.9 by the interval would be tomorrow at best; the day
        // turns now.
        CLOCK.store(T0 + DAY_NS, std::sync::atomic::Ordering::SeqCst);
        eventually("the new day is not stored", || {
            count(&d.store, "maturity_daily") == 2
        })
        .await;
        // The next read of the clock is a day check after the attempt
        // ended. Three more day checks then store nothing: each stored
        // snapshot would move the generation on.
        clock_reads(1).await;
        let stored = d.jobs.runtime.generation();
        clock_reads(3).await;
        assert_eq!(d.jobs.runtime.generation(), stored, "stored once");
        assert_eq!(count(&d.store, "maturity_daily"), 2, "one row per day");
        shutdown.cancel();
        wait(task, "score jobs").await;
        d.stop().await;
    }

    /// The spawned task syncs the register, snapshots at once (not after
    /// the first interval), and stops on shutdown so the writer can drain.
    #[tokio::test]
    async fn the_spawned_jobs_sync_snapshot_and_stop_on_shutdown() {
        let d = daemon(Some(ORG), Some(Duration::from_secs(3600)));
        let shutdown = CancellationToken::new();
        let task = spawn_score_jobs(d.jobs.clone(), d.ingest.clone(), shutdown.clone());
        eventually("no snapshot stored", || {
            count(&d.store, "requirement_status_daily") > 0
        })
        .await;
        assert_eq!(count(&d.store, "ict_functions"), 2);
        shutdown.cancel();
        wait(task, "score jobs").await;
        d.stop().await;
    }

    #[tokio::test]
    async fn a_disabled_snapshot_still_syncs_the_register_and_then_ends() {
        let d = daemon(Some(ORG), None);
        let shutdown = CancellationToken::new();
        let task = spawn_score_jobs(d.jobs.clone(), d.ingest.clone(), shutdown);
        wait(task, "score jobs").await;
        assert_eq!(count(&d.store, "ict_functions"), 2);
        assert_eq!(count(&d.store, "requirement_status_daily"), 0);
        d.stop().await;
    }

    /// Shutdown does not wait for a snapshot that is still running.
    #[tokio::test]
    async fn a_running_job_is_given_up_on_shutdown() {
        let shutdown = CancellationToken::new();
        assert_eq!(until_cancelled(async { 7 }, &shutdown).await, Some(7));

        let never = std::future::pending::<u8>();
        let stop = shutdown.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            stop.cancel();
        });
        let given_up =
            tokio::time::timeout(Duration::from_secs(5), until_cancelled(never, &shutdown))
                .await
                .expect("shutdown did not end the job");
        assert_eq!(given_up, None);
        // Once cancelled, nothing is started.
        assert_eq!(until_cancelled(async { 7 }, &shutdown).await, None);
    }

    #[test]
    fn the_intervals_have_defaults_and_honour_overrides() {
        let _guard = crate::test_support::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        type Read = fn() -> Option<Duration>;
        let cases: [(&str, Read, Duration); 2] = [
            (INTERVAL_ENV, snapshot_interval_from_env, DEFAULT_INTERVAL),
            (RELOAD_ENV, reload_interval_from_env, DEFAULT_RELOAD),
        ];
        for (key, read, default) in cases {
            std::env::remove_var(key);
            assert_eq!(read(), Some(default), "{key}");
            for off in ["", "0", "off", "OFF"] {
                std::env::set_var(key, off);
                assert_eq!(read(), None, "{key}={off:?} must disable");
            }
            std::env::set_var(key, "6h");
            assert_eq!(read(), Some(Duration::from_secs(21_600)), "{key}");
            std::env::set_var(key, "bogus");
            assert_eq!(read(), Some(default), "{key}");
            std::env::remove_var(key);
        }
        assert_eq!(DEFAULT_RELOAD, Duration::from_secs(60));
    }
}
