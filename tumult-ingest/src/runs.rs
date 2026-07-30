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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;
use tumult_core::runner::{ActivityExecutor, RunConfig};
use tumult_core::types::{Experiment, ExperimentStatus, Journal};
use tumult_lake::{rollback_status, run_state, NewRun, Store, Writer};

use crate::{Batch, IngestWriter};

/// Queue sizing. `TUMULTD_RUN_CONCURRENCY` (default 2) bounds concurrently
/// executing experiments; `TUMULTD_RUN_QUEUE_DEPTH` (default 32) bounds
/// runs waiting for a worker — enqueue beyond that is rejected (429 at the
/// API), never silently queued.
#[derive(Clone, Copy, Debug)]
pub struct RunQueueConfig {
    pub concurrency: usize,
    pub queue_depth: usize,
}

impl Default for RunQueueConfig {
    fn default() -> Self {
        Self {
            concurrency: 2,
            queue_depth: 32,
        }
    }
}

impl RunQueueConfig {
    /// From `TUMULTD_RUN_CONCURRENCY` / `TUMULTD_RUN_QUEUE_DEPTH`, falling
    /// back to defaults on unset/invalid values.
    #[must_use]
    pub fn from_env() -> Self {
        let default = Self::default();
        let parse = |key: &str, fallback: usize| {
            std::env::var(key)
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .filter(|&n| n > 0)
                .unwrap_or(fallback)
        };
        Self {
            concurrency: parse("TUMULTD_RUN_CONCURRENCY", default.concurrency),
            queue_depth: parse("TUMULTD_RUN_QUEUE_DEPTH", default.queue_depth),
        }
    }
}

/// Builds the activity executor for one run from its injected
/// `TUMULT_CONFIG_*` / `TUMULT_SECRET_*` environment. Production wires
/// `tumult_exec::ProviderExecutor`; tests inject fakes.
pub type ExecutorFactory =
    Arc<dyn Fn(HashMap<String, String>) -> Arc<dyn ActivityExecutor> + Send + Sync>;

/// A run accepted by `POST /api/runs`: the validated definition plus the
/// template variables to resolve.
pub struct RunRequest {
    pub registry_id: String,
    pub definition_toon: String,
    pub vars: HashMap<String, String>,
}

/// Why an enqueue was rejected.
#[derive(Debug)]
pub enum EnqueueError {
    /// Waiting queue is at capacity — the API maps this to 429.
    Full,
    /// Persisting the run row failed.
    Store(String),
}

/// Why a stop request failed.
#[derive(Debug)]
pub enum StopError {
    NotFound,
    /// Already in a terminal state (the state value is included).
    Terminal(String),
    Store(String),
}

struct Shared {
    db_path: PathBuf,
    ingest: IngestWriter,
    /// Cancellation tokens of runs executing in this process, by run id.
    tokens: Mutex<HashMap<String, CancellationToken>>,
}

struct WorkItem {
    run_id: String,
    request: RunRequest,
    /// Held until the worker dequeues: bounds the waiting queue.
    _permit: tokio::sync::OwnedSemaphorePermit,
}

/// Cloneable handle to the run queue (mirrors [`IngestWriter`]).
#[derive(Clone)]
pub struct RunQueue {
    tx: mpsc::Sender<WorkItem>,
    waiting: Arc<Semaphore>,
    shared: Arc<Shared>,
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

/// Map the journal's experiment status onto the run state machine.
fn terminal_state(status: &ExperimentStatus) -> &'static str {
    match status {
        ExperimentStatus::Completed => run_state::PASSED,
        ExperimentStatus::Deviated => run_state::DEVIATED,
        ExperimentStatus::Failed => run_state::FAILED,
        ExperimentStatus::Aborted | ExperimentStatus::Interrupted | ExperimentStatus::Halted => {
            run_state::ABORTED
        }
    }
}

/// Parse, resolve and validate a run request: the exact pipeline the CLI's
/// `tumult run` applies (config/secrets resolve against the daemon's
/// environment). Returns the resolved experiment and the
/// `TUMULT_CONFIG_*` / `TUMULT_SECRET_*` environment to inject. Shared by
/// the worker (enqueue path) and the API (validate / dry-run).
///
/// # Errors
/// Returns a stage-prefixed message (`parse:` / `config:` / `secrets:` /
/// `template:` / `validate:`) on any failure.
pub fn prepare_run(
    definition_toon: &str,
    vars: &HashMap<String, String>,
) -> Result<(Experiment, HashMap<String, String>), String> {
    use tumult_core::engine::{
        apply_template_vars, build_config_env, build_secret_env, flatten_secrets, parse_experiment,
        resolve_config, resolve_secrets, validate_experiment,
    };
    let experiment = parse_experiment(definition_toon).map_err(|e| format!("parse: {e}"))?;
    let config = resolve_config(&experiment.configuration).map_err(|e| format!("config: {e}"))?;
    let secrets = resolve_secrets(&experiment.secrets).map_err(|e| format!("secrets: {e}"))?;
    let secrets_flat = flatten_secrets(&secrets);
    let experiment = if vars.is_empty() && config.is_empty() && secrets_flat.is_empty() {
        experiment
    } else {
        apply_template_vars(&experiment, vars, &config, &secrets_flat)
            .map_err(|e| format!("template: {e}"))?
    };
    validate_experiment(&experiment).map_err(|e| format!("validate: {e}"))?;
    let (config_env, _skipped) = build_config_env(&config);
    let (secret_env, _skipped) = build_secret_env(&secrets_flat);
    let mut injected = config_env;
    injected.extend(secret_env);
    Ok((experiment, injected))
}

/// Wire the experiment's declared controls into a registry sharing the run's
/// executor (mirrors the CLI's wiring).
fn build_controls(
    experiment: &Experiment,
    executor: &Arc<dyn ActivityExecutor>,
) -> Arc<tumult_core::controls::ControlRegistry> {
    let mut controls = tumult_core::controls::ControlRegistry::new();
    for control in &experiment.controls {
        controls.register(Box::new(tumult_core::controls::ProviderControl::new(
            control.clone(),
            Arc::clone(executor),
        )));
    }
    Arc::new(controls)
}

/// Current state of one run, read on a fresh read-only connection.
fn read_run_state(db_path: &Path, run_id: &str) -> Option<String> {
    let reader = Store::at(db_path).read_only().ok()?;
    let run = reader.run_get(run_id).ok()??;
    run["state"].as_str().map(str::to_string)
}

impl RunQueue {
    /// Spawn the worker pool on the current tokio runtime. Workers exit when
    /// every `RunQueue` clone is dropped and the channel closes.
    #[must_use]
    pub fn spawn(
        ingest: IngestWriter,
        db_path: PathBuf,
        config: RunQueueConfig,
        factory: ExecutorFactory,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<WorkItem>(config.queue_depth.max(1));
        let shared = Arc::new(Shared {
            db_path,
            ingest,
            tokens: Mutex::new(HashMap::new()),
        });
        // One receiver fan-out: wrap in a Mutex so N workers share it.
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        for _ in 0..config.concurrency.max(1) {
            let rx = Arc::clone(&rx);
            let shared = Arc::clone(&shared);
            let factory = Arc::clone(&factory);
            tokio::spawn(async move {
                loop {
                    let item = {
                        let mut guard = rx.lock().await;
                        guard.recv().await
                    };
                    let Some(item) = item else { break };
                    process(item, &shared, &factory).await;
                }
                tracing::info!("run queue worker exiting (channel closed)");
            });
        }
        Self {
            tx,
            waiting: Arc::new(Semaphore::new(config.queue_depth.max(1))),
            shared,
        }
    }

    /// Persist and queue a run. Rejects with [`EnqueueError::Full`] when the
    /// waiting queue is at capacity — before anything is persisted.
    ///
    /// # Errors
    /// See [`EnqueueError`].
    pub async fn enqueue(&self, request: RunRequest) -> Result<String, EnqueueError> {
        let permit = self
            .waiting
            .clone()
            .try_acquire_owned()
            .map_err(|_| EnqueueError::Full)?;
        let run_id = uuid::Uuid::new_v4().to_string();
        let params_json = if request.vars.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&request.vars).unwrap_or_default())
        };
        let new_run = NewRun {
            id: run_id.clone(),
            registry_id: request.registry_id.clone(),
            params_json,
            queued_at_ns: now_ns(),
        };
        exec_write(&self.shared.ingest, move |writer| {
            writer.insert_run(&new_run).map_err(|e| e.to_string())
        })
        .await
        .map_err(EnqueueError::Store)?;
        let item = WorkItem {
            run_id: run_id.clone(),
            request,
            _permit: permit,
        };
        // The channel capacity matches the semaphore, so this cannot block
        // meaningfully; a closed channel means shutdown.
        if self.tx.send(item).await.is_err() {
            return Err(EnqueueError::Store("run queue stopped".into()));
        }
        Ok(run_id)
    }

    /// E-stop a run: cancel its token (the runner stops before the next
    /// activity and runs rollbacks) and record `stopping`. Runs still
    /// waiting are cancelled before they start.
    ///
    /// # Errors
    /// See [`StopError`].
    pub async fn stop(&self, run_id: &str) -> Result<(), StopError> {
        let token = self
            .shared
            .tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(run_id)
            .cloned();
        if let Some(token) = token {
            token.cancel();
            let id = run_id.to_string();
            exec_write(&self.shared.ingest, move |writer| {
                writer
                    .set_run_state_with(&id, run_state::STOPPING, Some("stop_requested"), None)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(StopError::Store)?;
            return Ok(());
        }
        match read_run_state(&self.shared.db_path, run_id) {
            None => Err(StopError::NotFound),
            Some(state) if run_state::TERMINAL.contains(&state.as_str()) => {
                Err(StopError::Terminal(state))
            }
            Some(_) => {
                // Waiting (queued/validating) but no token yet: cancel before
                // start — the worker re-checks state after dequeue and skips.
                let id = run_id.to_string();
                exec_write(&self.shared.ingest, move |writer| {
                    writer
                        .finish_run(
                            &id,
                            run_state::ABORTED,
                            None,
                            Some(rollback_status::NOT_NEEDED),
                            Some("cancelled before start"),
                        )
                        .map_err(|e| e.to_string())
                })
                .await
                .map_err(StopError::Store)
            }
        }
    }
}

/// One worker pass over a dequeued run: validate, execute, record.
async fn process(item: WorkItem, shared: &Shared, factory: &ExecutorFactory) {
    let WorkItem {
        run_id,
        request,
        _permit: permit,
    } = item;
    // Dequeued: the waiting-queue slot frees now, not when the run ends.
    drop(permit);
    let ingest = &shared.ingest;

    // The run may have been cancelled while waiting.
    if read_run_state(&shared.db_path, &run_id).as_deref() != Some(run_state::QUEUED) {
        return;
    }

    let id = run_id.clone();
    let _ = exec_write(ingest, move |writer| {
        writer
            .set_run_state(&id, run_state::VALIDATING)
            .map_err(|e| e.to_string())
    })
    .await;

    let (experiment, injected_env) = match prepare_run(&request.definition_toon, &request.vars) {
        Ok(prepared) => prepared,
        Err(e) => {
            let id = run_id.clone();
            let _ = exec_write(ingest, move |writer| {
                writer
                    .finish_run(&id, run_state::FAILED, None, None, Some(&e))
                    .map_err(|e| e.to_string())
            })
            .await;
            return;
        }
    };

    let id = run_id.clone();
    let _ = exec_write(ingest, move |writer| {
        writer
            .mark_run_started(&id, None)
            .map_err(|e| e.to_string())
    })
    .await;

    let token = CancellationToken::new();
    shared
        .tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(run_id.clone(), token.clone());

    let executor = factory(injected_env);
    let controls = build_controls(&experiment, &executor);
    let config = RunConfig {
        cancellation_token: Some(token),
        ..RunConfig::default()
    };
    let run_experiment = {
        let experiment = experiment.clone();
        move || tumult_core::runner::run_experiment(&experiment, &executor, &controls, &config)
    };
    let result = tokio::task::spawn_blocking(run_experiment).await;
    shared
        .tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&run_id);

    let journal = match result {
        Ok(Ok(journal)) => journal,
        Ok(Err(e)) => {
            record_failure(ingest, &run_id, &format!("runner: {e}")).await;
            return;
        }
        Err(e) => {
            record_failure(ingest, &run_id, &format!("runner task: {e}")).await;
            return;
        }
    };

    record_completion(ingest, &run_id, &experiment, journal).await;
}

/// Terminal state + journal ingest for a finished run.
async fn record_completion(
    ingest: &IngestWriter,
    run_id: &str,
    experiment: &Experiment,
    journal: Journal,
) {
    let state = terminal_state(&journal.status).to_string();
    let rb = if journal.rollback_results.is_empty() {
        rollback_status::NOT_NEEDED
    } else if journal.rollback_failures == 0 {
        rollback_status::COMPLETED
    } else {
        rollback_status::FAILED
    }
    .to_string();
    let experiment_id = journal.experiment_id.clone();
    let id = run_id.to_string();
    let _ = exec_write(ingest, move |writer| {
        writer
            .finish_run(&id, &state, Some(&experiment_id), Some(&rb), None)
            .map_err(|e| e.to_string())
    })
    .await;

    // Journal into the analytics tables on the same single writer.
    let experiment = experiment.clone();
    let _ = exec_write(ingest, move |writer| {
        writer
            .ingest_journal(&journal, Some(&experiment))
            .map(|_| ())
            .map_err(|e| e.to_string())
    })
    .await;
}

async fn record_failure(ingest: &IngestWriter, run_id: &str, error: &str) {
    let id = run_id.to_string();
    let error = error.to_string();
    let _ = exec_write(ingest, move |writer| {
        writer
            .finish_run(&id, run_state::FAILED, None, None, Some(&error))
            .map_err(|e| e.to_string())
    })
    .await;
}

/// Rollback outcome of one orphaned run.
enum OrphanOutcome {
    /// Never started executing — nothing to unwind.
    NothingExecuted,
    RolledBack,
    RollbackFailed(String),
}

/// Reconcile runs left active by a previous process lifetime. Called once at
/// daemon startup, before the servers accept traffic. Returns the number of
/// orphaned runs processed.
///
/// Fault cleanup is the primary duty: rollbacks are attempted even when the
/// state/audit writes themselves fail (a store error never skips a rollback
/// — the fault may still be live; write failures are logged and the run row
/// keeps its active state so the next start retries).
///
/// # Errors
/// Returns an error if the store cannot be read.
pub async fn reconcile_orphans(
    ingest: &IngestWriter,
    db_path: &Path,
    factory: &ExecutorFactory,
) -> Result<usize, String> {
    let orphans = {
        let reader = Store::at(db_path)
            .read_only()
            .map_err(|e| format!("open store read-only: {e}"))?;
        reader.active_runs().map_err(|e| e.to_string())?
    };
    if orphans.is_empty() {
        return Ok(0);
    }
    let count = orphans.len();
    for orphan in orphans {
        let run_id = orphan["id"].as_str().unwrap_or_default().to_string();
        let prior = orphan["state"].as_str().unwrap_or_default().to_string();
        let toon = orphan["definition_toon"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        tracing::warn!(run_id = %run_id, prior_state = %prior, "orphaned run from previous process lifetime");

        let id = run_id.clone();
        // Best-effort: a broken store must never skip the rollback — the
        // fault may still be live in the target system. The run row keeps
        // its active state, so the next daemon start retries.
        if let Err(e) = exec_write(ingest, move |writer| {
            writer
                .set_run_state_with(
                    &id,
                    run_state::ORPHANED,
                    Some("orphan_detected"),
                    Some("daemon restarted; run was owned by a previous process"),
                )
                .map_err(|e| e.to_string())
        })
        .await
        {
            tracing::error!(run_id = %run_id, error = %e, "orphan state write failed; attempting rollback anyway");
        }

        let outcome = if prior == run_state::RUNNING || prior == run_state::STOPPING {
            attempt_orphan_rollback(ingest, &run_id, &toon, factory).await
        } else {
            OrphanOutcome::NothingExecuted
        };

        let id = run_id.clone();
        if let Err(e) = exec_write(ingest, move |writer| {
            let result = match &outcome {
                OrphanOutcome::NothingExecuted => writer.finish_run(
                    &id,
                    run_state::ABORTED,
                    None,
                    Some(rollback_status::NOT_NEEDED),
                    Some("orphaned before execution"),
                ),
                OrphanOutcome::RolledBack => writer.finish_run(
                    &id,
                    run_state::ABORTED,
                    None,
                    Some(rollback_status::COMPLETED),
                    Some("orphaned; rollback completed after restart"),
                ),
                OrphanOutcome::RollbackFailed(e) => writer.finish_run(
                    &id,
                    run_state::ROLLBACK_PENDING,
                    None,
                    Some(rollback_status::FAILED),
                    Some(e),
                ),
            };
            result.map_err(|e| e.to_string())
        })
        .await
        {
            tracing::error!(run_id = %run_id, error = %e, "orphan terminal state write failed");
        }
    }
    Ok(count)
}

/// Run the rollback phase of an orphaned run's definition; audit the attempt.
async fn attempt_orphan_rollback(
    ingest: &IngestWriter,
    run_id: &str,
    toon: &str,
    factory: &ExecutorFactory,
) -> OrphanOutcome {
    let id = run_id.to_string();
    let _ = exec_write(ingest, move |writer| {
        writer
            .insert_run_audit(&id, "rollback_started", Some("orphan recovery"))
            .map_err(|e| e.to_string())
    })
    .await;

    let experiment = match tumult_core::engine::parse_experiment(toon) {
        Ok(exp) => exp,
        Err(e) => return OrphanOutcome::RollbackFailed(format!("definition unparseable: {e}")),
    };
    // Orphan recovery cannot re-resolve secrets from the crashed run's
    // context; rollbacks run with the daemon's current environment.
    let executor = factory(HashMap::new());
    let controls = build_controls(&experiment, &executor);
    let results = tokio::task::spawn_blocking(move || {
        tumult_core::runner::run_orphan_rollback(&experiment, &executor, &controls)
    })
    .await;

    let (event, outcome) = match results {
        Ok(results) => {
            let failures: Vec<String> = results
                .iter()
                .filter(|r| r.status != tumult_core::types::ActivityStatus::Succeeded)
                .map(|r| r.name.clone())
                .collect();
            if failures.is_empty() {
                ("rollback_completed", OrphanOutcome::RolledBack)
            } else {
                (
                    "rollback_failed",
                    OrphanOutcome::RollbackFailed(format!(
                        "rollback activities failed: {}",
                        failures.join(", ")
                    )),
                )
            }
        }
        Err(e) => (
            "rollback_failed",
            OrphanOutcome::RollbackFailed(format!("rollback task: {e}")),
        ),
    };
    let id = run_id.to_string();
    let _ = exec_write(ingest, move |writer| {
        writer
            .insert_run_audit(&id, event, None)
            .map_err(|e| e.to_string())
    })
    .await;
    outcome
}

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tumult_core::runner::ActivityOutcome;
    use tumult_core::types::Activity;
    use tumult_lake::RegisteredDefinition;

    /// Three method steps plus one rollback, native providers (the test
    /// executor intercepts everything regardless of provider).
    const TEST_TOON: &str = r#"
title: queue test experiment
method[3]:
  - name: action-1
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
  - name: action-2
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
  - name: action-3
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
rollbacks[1]:
  - name: rollback-1
    activity_type: action
    provider:
      type: native
      plugin: test
      function: noop
"#;

    /// Records executed activity names; each execution sleeps `delay`.
    struct RecordingExecutor {
        executed: Arc<Mutex<Vec<String>>>,
        delay: Duration,
    }
    impl ActivityExecutor for RecordingExecutor {
        fn execute(&self, activity: &Activity) -> ActivityOutcome {
            self.executed.lock().unwrap().push(activity.name.clone());
            std::thread::sleep(self.delay);
            ActivityOutcome {
                success: true,
                output: Some("ok".into()),
                error: None,
                duration_ms: 0,
            }
        }
    }

    fn recording_factory(executed: &Arc<Mutex<Vec<String>>>, delay: Duration) -> ExecutorFactory {
        let executed = Arc::clone(executed);
        Arc::new(move |_| {
            Arc::new(RecordingExecutor {
                executed: Arc::clone(&executed),
                delay,
            })
        })
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        db_path: PathBuf,
        ingest: IngestWriter,
        executed: Arc<Mutex<Vec<String>>>,
    }

    async fn fixture() -> Fixture {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_path = tmp.path().join("kronika.duckdb");
        let store = Store::open(&db_path).unwrap();
        let (ingest, _task) = IngestWriter::spawn(store.writer().unwrap(), 64);
        let executed = Arc::new(Mutex::new(Vec::new()));
        // The registry write rides the same channel as production.
        exec_write(&ingest, move |writer| {
            writer
                .register_definition(&RegisteredDefinition {
                    id: "reg-1".into(),
                    name: "queue test experiment".into(),
                    definition_toon: TEST_TOON.into(),
                    content_hash: "hash-1".into(),
                    registered_at_ns: 1,
                    registered_by: Some("test".into()),
                })
                .map_err(|e| e.to_string())
        })
        .await
        .unwrap();
        Fixture {
            _tmp: tmp,
            db_path,
            ingest,
            executed,
        }
    }

    fn request() -> RunRequest {
        RunRequest {
            registry_id: "reg-1".into(),
            definition_toon: TEST_TOON.into(),
            vars: HashMap::new(),
        }
    }

    /// Poll the run's state until `want` or timeout (5s).
    async fn await_state(fx: &Fixture, run_id: &str, want: &str) -> String {
        for _ in 0..100 {
            if let Some(state) = read_run_state(&fx.db_path, run_id) {
                if state == want {
                    return state;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("run {run_id} never reached state {want}");
    }

    async fn await_terminal(fx: &Fixture, run_id: &str) -> String {
        for _ in 0..100 {
            if let Some(state) = read_run_state(&fx.db_path, run_id) {
                if run_state::TERMINAL.contains(&state.as_str()) {
                    return state;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("run {run_id} never reached a terminal state");
    }

    fn run_row(fx: &Fixture, run_id: &str) -> serde_json::Value {
        Store::at(&fx.db_path)
            .read_only()
            .unwrap()
            .run_get(run_id)
            .unwrap()
            .unwrap()
    }

    fn audit_events(fx: &Fixture, run_id: &str) -> Vec<String> {
        Store::at(&fx.db_path)
            .read_only()
            .unwrap()
            .run_audit_trail(run_id)
            .unwrap()
            .iter()
            .filter_map(|e| e["event"].as_str().map(str::to_string))
            .collect()
    }

    #[tokio::test]
    async fn run_executes_to_passed_and_ingests_journal() {
        let fx = fixture().await;
        let queue = RunQueue::spawn(
            fx.ingest.clone(),
            fx.db_path.clone(),
            RunQueueConfig {
                concurrency: 1,
                queue_depth: 4,
            },
            recording_factory(&fx.executed, Duration::from_millis(5)),
        );

        let run_id = queue.enqueue(request()).await.unwrap();
        assert_eq!(await_terminal(&fx, &run_id).await, run_state::PASSED);

        let run = run_row(&fx, &run_id);
        assert_eq!(
            run["rollback_status"],
            serde_json::json!(rollback_status::NOT_NEEDED)
        );
        let experiment_id = run["experiment_id"].as_str().unwrap();
        assert!(!experiment_id.is_empty());
        assert_eq!(
            fx.executed.lock().unwrap().as_slice(),
            ["action-1", "action-2", "action-3"]
        );
        // The journal landed in the analytics tables via the same writer.
        let rows = Store::at(&fx.db_path)
            .read_only()
            .unwrap()
            .query_json_rows("SELECT experiment_id, status FROM experiments")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["experiment_id"], serde_json::json!(experiment_id));
    }

    #[tokio::test]
    async fn enqueue_rejects_beyond_queue_depth() {
        let fx = fixture().await;
        let queue = RunQueue::spawn(
            fx.ingest.clone(),
            fx.db_path.clone(),
            RunQueueConfig {
                concurrency: 1,
                queue_depth: 1,
            },
            recording_factory(&fx.executed, Duration::from_millis(200)),
        );

        let r1 = queue.enqueue(request()).await.unwrap();
        await_state(&fx, &r1, run_state::RUNNING).await;
        // r2 takes the only waiting permit; r3 must be rejected, not queued.
        let r2 = queue.enqueue(request()).await.unwrap();
        assert!(matches!(
            queue.enqueue(request()).await,
            Err(EnqueueError::Full)
        ));

        // Only the two accepted runs were persisted.
        let runs = Store::at(&fx.db_path)
            .read_only()
            .unwrap()
            .runs(None, 10)
            .unwrap();
        assert_eq!(runs.len(), 2);

        assert_eq!(await_terminal(&fx, &r1).await, run_state::PASSED);
        assert_eq!(await_terminal(&fx, &r2).await, run_state::PASSED);
    }

    #[tokio::test]
    async fn stop_mid_method_runs_rollback_and_aborts() {
        let fx = fixture().await;
        let queue = RunQueue::spawn(
            fx.ingest.clone(),
            fx.db_path.clone(),
            RunQueueConfig {
                concurrency: 1,
                queue_depth: 4,
            },
            recording_factory(&fx.executed, Duration::from_millis(250)),
        );

        let run_id = queue.enqueue(request()).await.unwrap();
        // Wait until the first activity finished (second is sleeping).
        for _ in 0..100 {
            if !fx.executed.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        queue.stop(&run_id).await.unwrap();
        assert_eq!(await_terminal(&fx, &run_id).await, run_state::ABORTED);

        let events = audit_events(&fx, &run_id);
        assert!(events.contains(&"stop_requested".to_string()), "{events:?}");
        // The e-stop unwound the active fault via the rollback path.
        let executed = fx.executed.lock().unwrap();
        assert!(executed.contains(&"rollback-1".to_string()), "{executed:?}");
        // …and never ran the final method step.
        assert!(!executed.contains(&"action-3".to_string()), "{executed:?}");
        let run = run_row(&fx, &run_id);
        assert_eq!(
            run["rollback_status"],
            serde_json::json!(rollback_status::COMPLETED)
        );
    }

    #[tokio::test]
    async fn stop_queued_run_cancels_before_start() {
        let fx = fixture().await;
        let queue = RunQueue::spawn(
            fx.ingest.clone(),
            fx.db_path.clone(),
            RunQueueConfig {
                concurrency: 1,
                queue_depth: 4,
            },
            recording_factory(&fx.executed, Duration::from_millis(150)),
        );

        let r1 = queue.enqueue(request()).await.unwrap();
        await_state(&fx, &r1, run_state::RUNNING).await;
        let r2 = queue.enqueue(request()).await.unwrap();
        queue.stop(&r2).await.unwrap();

        assert_eq!(await_terminal(&fx, &r2).await, run_state::ABORTED);
        let run = run_row(&fx, &r2);
        assert_eq!(run["error"], serde_json::json!("cancelled before start"));
        assert_eq!(await_terminal(&fx, &r1).await, run_state::PASSED);
        // r2 never executed anything: only r1's three method steps ran.
        assert_eq!(fx.executed.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn stop_unknown_or_terminal_run_errors() {
        let fx = fixture().await;
        let queue = RunQueue::spawn(
            fx.ingest.clone(),
            fx.db_path.clone(),
            RunQueueConfig {
                concurrency: 1,
                queue_depth: 4,
            },
            recording_factory(&fx.executed, Duration::from_millis(5)),
        );
        assert!(matches!(queue.stop("nope").await, Err(StopError::NotFound)));

        let run_id = queue.enqueue(request()).await.unwrap();
        await_terminal(&fx, &run_id).await;
        assert!(matches!(
            queue.stop(&run_id).await,
            Err(StopError::Terminal(_))
        ));
    }

    #[tokio::test]
    async fn orphan_reconciliation_rolls_back_and_audits() {
        let fx = fixture().await;
        // Simulate a crash: a run left `running` by a dead process.
        exec_write(&fx.ingest, move |writer| {
            writer
                .insert_run(&NewRun {
                    id: "run-orphan".into(),
                    registry_id: "reg-1".into(),
                    params_json: None,
                    queued_at_ns: 1,
                })
                .map_err(|e| e.to_string())
        })
        .await
        .unwrap();
        exec_write(&fx.ingest, move |writer| {
            writer
                .mark_run_started("run-orphan", None)
                .map_err(|e| e.to_string())
        })
        .await
        .unwrap();

        let factory = recording_factory(&fx.executed, Duration::from_millis(5));
        let count = reconcile_orphans(&fx.ingest, &fx.db_path, &factory)
            .await
            .unwrap();
        assert_eq!(count, 1);

        let run = run_row(&fx, "run-orphan");
        assert_eq!(run["state"], serde_json::json!(run_state::ABORTED));
        assert_eq!(
            run["rollback_status"],
            serde_json::json!(rollback_status::COMPLETED)
        );
        // Only the rollback executed — the orphaned method never re-ran.
        assert_eq!(fx.executed.lock().unwrap().as_slice(), ["rollback-1"]);
        let events = audit_events(&fx, "run-orphan");
        for want in ["orphan_detected", "rollback_started", "rollback_completed"] {
            assert!(events.contains(&want.to_string()), "{events:?}");
        }
    }

    #[tokio::test]
    async fn orphan_never_started_aborts_without_rollback() {
        let fx = fixture().await;
        exec_write(&fx.ingest, move |writer| {
            writer
                .insert_run(&NewRun {
                    id: "run-queued".into(),
                    registry_id: "reg-1".into(),
                    params_json: None,
                    queued_at_ns: 1,
                })
                .map_err(|e| e.to_string())
        })
        .await
        .unwrap();

        let factory = recording_factory(&fx.executed, Duration::from_millis(5));
        let count = reconcile_orphans(&fx.ingest, &fx.db_path, &factory)
            .await
            .unwrap();
        assert_eq!(count, 1);

        let run = run_row(&fx, "run-queued");
        assert_eq!(run["state"], serde_json::json!(run_state::ABORTED));
        assert_eq!(
            run["rollback_status"],
            serde_json::json!(rollback_status::NOT_NEEDED)
        );
        assert!(fx.executed.lock().unwrap().is_empty());
        let events = audit_events(&fx, "run-queued");
        assert!(events.contains(&"orphan_detected".to_string()));
        assert!(!events.contains(&"rollback_started".to_string()));
    }
}
