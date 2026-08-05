//! The queue core: configuration, request types, the spawn/shutdown
//! lifecycle and the enqueue paths. Dispatch of approved runs and e-stop
//! live in [`dispatch`] and [`stop`].

mod dispatch;
mod stop;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;
use tumult_core::runner::ActivityExecutor;
use tumult_core::types::Experiment;
use tumult_lake::{approval_pin, ApprovalRequest, CanonicalPin, NewRun};

use super::worker::{process, sweep_expired_approvals};
use super::{exec_write, now_ns, Shared, WorkItem};
use crate::approvals::Tier;
use crate::IngestWriter;

/// Queue sizing. `TUMULTD_RUN_CONCURRENCY` (default 2) bounds concurrently
/// executing experiments; `TUMULTD_RUN_QUEUE_DEPTH` (default 32) bounds
/// runs waiting for a worker — enqueue beyond that is rejected (429 at the
/// API), never silently queued. `TUMULTD_APPROVAL_SWEEP_S` (default 60) is
/// the approval-TTL sweeper interval (T10).
#[derive(Clone, Copy, Debug)]
pub struct RunQueueConfig {
    pub concurrency: usize,
    pub queue_depth: usize,
    pub sweep_interval: std::time::Duration,
}

impl Default for RunQueueConfig {
    fn default() -> Self {
        Self {
            concurrency: 2,
            queue_depth: 32,
            sweep_interval: std::time::Duration::from_secs(60),
        }
    }
}

impl RunQueueConfig {
    /// From `TUMULTD_RUN_CONCURRENCY` / `TUMULTD_RUN_QUEUE_DEPTH` /
    /// `TUMULTD_APPROVAL_SWEEP_S`, falling back to defaults on unset/invalid
    /// values.
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
            sweep_interval: std::time::Duration::from_secs(parse(
                "TUMULTD_APPROVAL_SWEEP_S",
                default.sweep_interval.as_secs().max(1) as usize,
            ) as u64),
        }
    }
}

/// Builds the activity executor for one run from its injected
/// `TUMULT_CONFIG_*` / `TUMULT_SECRET_*` environment. Production wires
/// `tumult_exec::ProviderExecutor`; tests inject fakes.
pub type ExecutorFactory =
    Arc<dyn Fn(HashMap<String, String>) -> Arc<dyn ActivityExecutor> + Send + Sync>;

/// A run accepted by `POST /api/runs`: the validated definition plus the
/// template variables to resolve. `env` and `target` are the
/// approval-relevant execution context — both are covered by the canonical
/// pin (T10, ADR-013).
pub struct RunRequest {
    pub registry_id: String,
    pub definition_toon: String,
    pub vars: HashMap<String, String>,
    /// Target environment name (tier-classified by
    /// [`crate::approvals::env_class`]); `"dev"` when the caller omits it.
    pub env: String,
    /// Optional target selector (service/host/…), pinning what the fault
    /// aims at.
    pub target: Option<String>,
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

/// Why dispatching an approved run failed.
#[derive(Debug)]
pub enum DispatchError {
    /// The run does not exist or is not waiting for approval.
    NotPending,
    /// Waiting queue is at capacity — retry before the approval TTL lapses.
    Full,
    /// The approval itself does not clear dispatch (reason included):
    /// rejected, quorum short, consumed, or expired.
    Approval(String),
    /// Reading or writing the store failed.
    Store(String),
}

/// Cloneable handle to the run queue (mirrors [`IngestWriter`]).
#[derive(Clone)]
pub struct RunQueue {
    tx: mpsc::Sender<WorkItem>,
    waiting: Arc<Semaphore>,
    shared: Arc<Shared>,
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
pub(super) fn build_controls(
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

impl RunQueue {
    /// Spawn the worker pool on the current tokio runtime. Workers exit when
    /// every `RunQueue` clone is dropped and the channel closes; the approval
    /// sweeper runs until [`RunQueue::shutdown`] is called.
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
            shutdown: CancellationToken::new(),
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
        // Approval TTL sweeper (T10): gated runs whose approval lapses
        // before dispatch transition to the terminal `expired` state. The
        // approve path also checks the TTL lazily; this task is what makes
        // a lapsed request terminal rather than merely undispatchable. It
        // exits on `RunQueue::shutdown` so the shared `IngestWriter` clone
        // is released before the daemon drains the writer channel.
        {
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                let mut interval = tokio::time::interval(config.sweep_interval);
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                // interval's first tick fires immediately; consume it so the
                // first sweep runs one full interval after boot (approve and
                // break-glass also check the TTL lazily — nothing is
                // dispatchable in the gap either way).
                interval.tick().await;
                loop {
                    tokio::select! {
                        _ = interval.tick() => sweep_expired_approvals(&shared).await,
                        () = shared.shutdown.cancelled() => {
                            tracing::info!("approval sweeper exiting (shutdown)");
                            break;
                        }
                    }
                }
            });
        }
        Self {
            tx,
            waiting: Arc::new(Semaphore::new(config.queue_depth.max(1))),
            shared,
        }
    }

    /// Signal background tasks (the approval sweeper) to stop. The daemon
    /// calls this before draining the ingest writer: the sweeper holds the
    /// shared `IngestWriter` clone, and the writer channel closes only once
    /// every clone is dropped.
    pub fn shutdown(&self) {
        self.shared.shutdown.cancel();
    }

    /// Persist and queue a run. Rejects with [`EnqueueError::Full`] when the
    /// waiting queue is at capacity — before anything is persisted. `actor`
    /// is the authenticated identity behind the enqueue, recorded on the
    /// `enqueued` audit event (`None` when unauthenticated).
    ///
    /// # Errors
    /// See [`EnqueueError`].
    pub async fn enqueue(
        &self,
        request: RunRequest,
        actor: Option<String>,
    ) -> Result<String, EnqueueError> {
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
            actor,
        };
        exec_write(&self.shared.ingest, move |writer| {
            writer.insert_run(&new_run).map_err(|e| e.to_string())
        })
        .await
        .map_err(EnqueueError::Store)?;
        let item = WorkItem {
            run_id: run_id.clone(),
            request,
            approval_pin: None,
            _permit: permit,
        };
        // The channel capacity matches the semaphore, so this cannot block
        // meaningfully; a closed channel means shutdown.
        if self.tx.send(item).await.is_err() {
            return Err(EnqueueError::Store("run queue stopped".into()));
        }
        Ok(run_id)
    }

    /// Persist a gated run (tier T1–T3): the run row waits in
    /// `pending_approval` with its canonical pin, quorum and TTL recorded —
    /// nothing enters the worker channel until [`Self::dispatch_approved`]
    /// (T10, ADR-013). Returns the run id.
    ///
    /// # Errors
    /// See [`EnqueueError`].
    pub async fn request_gated(
        &self,
        request: RunRequest,
        tier: Tier,
        actor: Option<String>,
    ) -> Result<String, EnqueueError> {
        let run_id = uuid::Uuid::new_v4().to_string();
        let params: BTreeMap<String, String> = request.vars.clone().into_iter().collect();
        let pin = approval_pin(&CanonicalPin {
            definition_toon: &request.definition_toon,
            params: &params,
            env: &request.env,
            target: request.target.as_deref(),
        });
        let now = now_ns();
        let approval = ApprovalRequest {
            run_id: run_id.clone(),
            tier: tier.as_str().to_string(),
            pin_hash: pin.clone(),
            env: request.env.clone(),
            target: request.target.clone(),
            quorum_required: tier.quorum_required(),
            requested_by: actor.clone().unwrap_or_else(|| "synthetic".into()),
            requested_at_ns: now,
            expires_at_ns: now + tier.ttl_ns(),
        };
        let params_json = if request.vars.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&request.vars).unwrap_or_default())
        };
        let new_run = NewRun {
            id: run_id.clone(),
            registry_id: request.registry_id.clone(),
            params_json,
            queued_at_ns: now,
            actor,
        };
        let detail = format!(
            "tier {} quorum {} ttl {}h pin {}",
            approval.tier,
            approval.quorum_required,
            tier.ttl_ns() / 3_600_000_000_000,
            pin
        );
        exec_write(&self.shared.ingest, move |writer| {
            writer
                .insert_gated_run(&new_run, &approval, Some(&detail))
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(EnqueueError::Store)?;
        Ok(run_id)
    }
}
