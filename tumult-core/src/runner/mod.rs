//! Experiment runner — orchestrates five-phase execution lifecycle.
//!
//! The runner coordinates:
//! 1. Estimate recording (Phase 0)
//! 2. Optional probe-only observation (statistical acquisition remains external)
//! 3. Hypothesis evaluation (before)
//! 4. Method execution with during-phase sampling (Phase 2)
//! 5. Post-phase recovery measurement (Phase 3)
//! 6. Hypothesis evaluation (after)
//! 7. Rollback execution
//! 8. Analysis (Phase 4)
//! 9. Journal creation

use std::time::Duration;

use crate::execution::RollbackStrategy;
use crate::types::{Activity, Experiment, LoadConfig, LoadResult, LoadTool, VersionShare};

use thiserror::Error;
use tokio_util::sync::CancellationToken;

mod activity;
mod experiment;
mod gameday;
mod guard;
/// K6 load executor — spawns k6 and parses its summary into a `LoadResult`.
pub mod k6;
mod load;
pub mod load_profile;
pub(crate) mod nfr;
pub mod oha;
mod phases;
pub(crate) mod robustness;
mod sampler;
mod telemetry;

pub use experiment::{run_experiment, run_experiment_with_sampling, run_orphan_rollback};
pub use gameday::{run_gameday, run_gameday_with_wiring, ExperimentWiring};
pub use telemetry::epoch_nanos_now;

pub(crate) const TRACER_NAME: &str = "tumult-engine";

/// Errors produced by the runner before or during experiment orchestration.
#[derive(Error, Debug)]
pub enum RunnerError {
    /// The experiment's method section contains no steps.
    #[error("experiment has no method steps")]
    EmptyMethod,
    #[error("baseline-only requires at least one steady-state probe")]
    MissingBaselineProbes,
    /// A declared load could not start, or its tool has no adapter. No
    /// fault ran.
    #[error("load startup failed: {0}")]
    LoadStart(String),
    /// A `GameDay` declares a different number of experiments than were provided.
    #[error("gameday declares {declared} experiments but {provided} were provided")]
    ExperimentCountMismatch { declared: usize, provided: usize },
    /// The `recovery` block asks for a window the post phase cannot watch
    /// (see [`crate::types::RecoveryConfig::invalid_reason`]). No fault ran.
    #[error("recovery is invalid: {0}")]
    InvalidRecovery(String),
}

/// Probe sampling cadence for during-phase and post-phase collection.
///
/// The runner samples hypothesis probes concurrently with the method
/// (during phase) and again after it completes (post phase, to measure
/// recovery). Defaults are conservative: experiments without hypothesis
/// probes skip sampling entirely, and probes that are already back within
/// tolerance finish the post phase after a single round, so simple
/// experiments see no added latency.
#[derive(Debug, Clone)]
pub struct SamplingConfig {
    /// Pause between probe sampling rounds of the during phase, and
    /// between two reads of the guards.
    pub interval: Duration,
    /// Upper bound on during-phase sampling rounds, guarding against
    /// unbounded sample growth for long-running methods.
    pub max_during_samples: u32,
    /// How long the post phase keeps sampling while waiting for probes to
    /// return within tolerance (recovery) before giving up.
    pub recovery_timeout: Duration,
    /// Pause between probe sampling rounds of the post phase.
    pub recovery_interval: Duration,
}

impl Default for SamplingConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            max_during_samples: 300,
            recovery_timeout: Duration::from_secs(30),
            recovery_interval: Duration::from_secs(1),
        }
    }
}

impl SamplingConfig {
    /// The cadence of a run of `experiment`: its `recovery` block sets how
    /// long the post phase watches and how often it reads the probes. An
    /// experiment without the block gets [`SamplingConfig::default`].
    ///
    /// # Errors
    ///
    /// Returns [`RunnerError::InvalidRecovery`] for a window the post phase
    /// cannot watch.
    pub fn for_experiment(experiment: &Experiment) -> Result<Self, RunnerError> {
        let Some(recovery) = experiment.recovery else {
            return Ok(Self::default());
        };
        if let Some(reason) = recovery.invalid_reason() {
            return Err(RunnerError::InvalidRecovery(reason));
        }
        Ok(Self {
            recovery_timeout: Duration::from_secs_f64(recovery.window_s),
            recovery_interval: Duration::from_secs_f64(recovery.interval_s),
            ..Self::default()
        })
    }
}

/// Outcome of executing a single activity via a provider.
#[derive(Debug, Clone)]
pub struct ActivityOutcome {
    /// Whether the activity succeeded.
    pub success: bool,
    /// Provider output captured on success, if any.
    pub output: Option<String>,
    /// Failure reason captured on error, if any.
    pub error: Option<String>,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
}

/// Trait for executing activities -- allows mocking in tests.
pub trait ActivityExecutor: Send + Sync {
    /// Executes a single activity and returns its outcome.
    fn execute(&self, activity: &Activity) -> ActivityOutcome;

    /// Executes a single activity, and ends it early when `stop` fires.
    ///
    /// The runner calls this for the steps of the method, with the token
    /// that a guard halt or a stop of the run cancels. A step that was ended
    /// this way did not succeed, and its error says that it was stopped.
    /// Rollbacks and the probes of the steady state go through
    /// [`execute`](Self::execute): a stop does not end the cleanup.
    ///
    /// The default cannot stop a step: it runs the step to its end.
    fn execute_until(&self, activity: &Activity, stop: &CancellationToken) -> ActivityOutcome {
        let _ = stop;
        self.execute(activity)
    }
}

/// Handle to a running load test process.
///
/// Returned by [`LoadExecutor::start`]. Call [`LoadExecutor::stop`]
/// to terminate the process and collect results.
pub struct LoadHandle {
    /// Opaque handle — implementations store process state here.
    pub inner: Box<dyn std::any::Any + Send>,
}

/// Trait for starting and stopping load test tools (k6, `JMeter`).
///
/// Implementations spawn a background process and parse metrics
/// from its output when stopped.
pub trait LoadExecutor: Send + Sync {
    /// Starts the load tool as a background process.
    ///
    /// # Errors
    ///
    /// Returns an error if the load tool binary is not found or fails to start.
    fn start(&self, config: &LoadConfig) -> Result<LoadHandle, String>;

    /// Stops the running load test and collects metrics.
    ///
    /// # Errors
    ///
    /// Returns an error if the process cannot be stopped or metrics cannot be parsed.
    fn stop(&self, handle: LoadHandle) -> Result<LoadResult, String>;

    /// Ends the load test at once because the run was halted or
    /// interrupted: nothing may keep hitting the target while a fault is
    /// still injected and rollbacks wait. Whatever had been measured is
    /// kept. The default is [`LoadExecutor::stop`], for tools whose stop
    /// never waits.
    ///
    /// # Errors
    ///
    /// Returns an error if the tool had measured nothing yet.
    fn abort(&self, handle: LoadHandle) -> Result<LoadResult, String> {
        self.stop(handle)
    }
}

/// The adapter for a configured load tool. `jmeter` is accepted by the
/// definition format but has no adapter, so asking for it is an error
/// rather than a silent fallback to k6. Validation asks here too, so such
/// a load is refused before the run.
///
/// # Errors
///
/// Returns an error naming the supported tools when `tool` has no adapter.
pub fn load_executor_for(tool: &LoadTool) -> Result<std::sync::Arc<dyn LoadExecutor>, String> {
    match tool {
        LoadTool::K6 => Ok(std::sync::Arc::new(k6::K6LoadExecutor)),
        LoadTool::Oha => Ok(std::sync::Arc::new(oha::OhaLoadExecutor::default())),
        LoadTool::Jmeter => Err("no load adapter for jmeter; supported tools: k6, oha".into()),
    }
}

/// Selects the probe-only measurement path or ordinary fault execution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BaselineMode {
    /// Runs the ordinary experiment using its declared static tolerances.
    #[default]
    Full,
    /// Measures hypothesis probes only; never runs controls, load or faults.
    Only,
}

/// What a run measured and when: the question a
/// [`TargetVersionResolver`] answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunWindow {
    /// The definition's `target.system`.
    pub system: String,
    /// The definition's `target.environment`.
    pub environment: Option<String>,
    /// The definition's `target.namespace` (`service.namespace`).
    pub namespace: Option<String>,
    /// The run's start and end; each moved outwards by
    /// [`VERSION_WINDOW_SKEW`] when the telemetry named no version
    /// between them.
    pub started_at_ns: i64,
    pub ended_at_ns: i64,
}

/// How far the window a resolver is asked about reaches beyond the run on
/// either side when the run's own window is silent: the tolerance for the
/// runner's clock and the target's disagreeing about when a request
/// happened.
pub const VERSION_WINDOW_SKEW: Duration = Duration::from_secs(2);

/// How a run waits for its target's telemetry to arrive before it reads
/// the version from it: the question is put again every `pause` until two
/// answers in a row count the same requests, or `deadline` has passed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VersionSettle {
    pub pause: Duration,
    pub deadline: Duration,
}

impl VersionSettle {
    /// Ask once and take the answer.
    pub const ONCE: Self = Self {
        pause: Duration::ZERO,
        deadline: Duration::ZERO,
    };
}

impl Default for VersionSettle {
    /// A second between questions, five seconds in all: an exporter
    /// batches its spans, so the last of a run's can arrive after it.
    fn default() -> Self {
        Self {
            pause: Duration::from_secs(1),
            deadline: Duration::from_secs(5),
        }
    }
}

/// Finds the version of the target that served a run, from the target's
/// own telemetry. Asked once, when the run ends, and only when the
/// definition declares a target without a version.
pub trait TargetVersionResolver: Send + Sync {
    /// Every `service.version` the target reported inside `window`, in
    /// the window's environment and namespace, with the requests each
    /// served; empty when its telemetry says nothing.
    ///
    /// # Errors
    ///
    /// Returns an error if the telemetry could not be read.
    fn resolve(&self, window: &RunWindow) -> Result<Vec<VersionShare>, String>;

    /// How long the run waits for the telemetry to settle.
    fn settle(&self) -> VersionSettle {
        VersionSettle::default()
    }
}

/// Ask `resolver` about `window` until two answers in a row count the
/// same requests or the resolver's deadline has passed, and return the
/// last answer.
///
/// # Errors
///
/// Returns the resolver's error.
pub fn settled_versions(
    resolver: &dyn TargetVersionResolver,
    window: &RunWindow,
) -> Result<Vec<VersionShare>, String> {
    settled_versions_unless(resolver, window, None)
}

/// [`settled_versions`] for a run that can be told to stop: once
/// `cancelled` is set the asking ends, inside a pause too, and the last
/// answer stands.
///
/// # Errors
///
/// Returns the resolver's error.
pub fn settled_versions_unless(
    resolver: &dyn TargetVersionResolver,
    window: &RunWindow,
    cancelled: Option<&CancellationToken>,
) -> Result<Vec<VersionShare>, String> {
    let requests = |seen: &[VersionShare]| -> u64 { seen.iter().map(|share| share.requests).sum() };
    let settle = resolver.settle();
    let asked_at = std::time::Instant::now();
    let mut last = resolver.resolve(window)?;
    while !settle.pause.is_zero() && asked_at.elapsed() + settle.pause <= settle.deadline {
        if !activity::wait_pause(Some(settle.pause.as_secs_f64()), cancelled) {
            break;
        }
        let next = resolver.resolve(window)?;
        let settled = requests(&next) == requests(&last);
        last = next;
        if settled {
            break;
        }
    }
    Ok(last)
}

/// Configuration for an experiment run.
///
/// Dry-run is handled before calling the runner.
pub struct RunConfig {
    /// Baseline-only measurement is enforced by the runner before any actions.
    pub baseline_mode: BaselineMode,
    /// When to execute rollbacks (defaults to `RollbackStrategy::OnDeviation`).
    pub rollback_strategy: RollbackStrategy,
    /// Optional cancellation token. When cancelled, the runner stops before
    /// dispatching waiting foreground/background activities, interrupts pauses,
    /// runs rollbacks for any fault
    /// already injected, and ends the run with `ExperimentStatus::Interrupted`
    /// — a cancelled run is never reported `Completed`.
    pub cancellation_token: Option<CancellationToken>,
    /// Optional parent OpenTelemetry context. When provided, the root
    /// `resilience.experiment` span is created as a child of this context,
    /// enabling cross-service trace linking (e.g. from an MCP tool span).
    pub parent_context: Option<opentelemetry::Context>,
    /// Optional load test executor for the declared `load`. When `None`,
    /// the runner uses the adapter of the tool the load names
    /// ([`load_executor_for`]); tests give a stand-in here. The runner
    /// starts the load in the background during method execution.
    pub load_executor: Option<std::sync::Arc<dyn LoadExecutor>>,
    /// Optional source of the target's observed version. Without one a
    /// run whose definition declares no `target.version` records none.
    pub version_resolver: Option<std::sync::Arc<dyn TargetVersionResolver>>,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            baseline_mode: BaselineMode::Full,
            rollback_strategy: RollbackStrategy::OnDeviation,
            cancellation_token: None,
            parent_context: None,
            load_executor: None,
            version_resolver: None,
        }
    }
}

#[cfg(test)]
mod tests;
