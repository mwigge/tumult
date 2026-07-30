//! Experiment runner — orchestrates five-phase execution lifecycle.
//!
//! The runner coordinates:
//! 1. Estimate recording (Phase 0)
//! 2. Baseline acquisition (Phase 1)
//! 3. Hypothesis evaluation (before)
//! 4. Method execution with during-phase sampling (Phase 2)
//! 5. Post-phase recovery measurement (Phase 3)
//! 6. Hypothesis evaluation (after)
//! 7. Rollback execution
//! 8. Analysis (Phase 4)
//! 9. Journal creation

use std::time::Duration;

use crate::execution::RollbackStrategy;
use crate::types::{Activity, LoadConfig, LoadResult};

use thiserror::Error;
use tokio_util::sync::CancellationToken;

mod activity;
mod experiment;
mod gameday;
mod guard;
pub mod k6;
mod load;
mod phases;
mod sampler;
mod telemetry;

pub use experiment::{run_experiment, run_experiment_with_sampling, run_orphan_rollback};
pub use gameday::{run_gameday, run_gameday_with_wiring, ExperimentWiring};
pub use telemetry::epoch_nanos_now;

pub(crate) const TRACER_NAME: &str = "tumult-engine";

#[derive(Error, Debug)]
pub enum RunnerError {
    #[error("experiment has no method steps")]
    EmptyMethod,
    #[error("gameday declares {declared} experiments but {provided} were provided")]
    ExperimentCountMismatch { declared: usize, provided: usize },
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
    /// Pause between probe sampling rounds in both phases.
    pub interval: Duration,
    /// Upper bound on during-phase sampling rounds, guarding against
    /// unbounded sample growth for long-running methods.
    pub max_during_samples: u32,
    /// How long the post phase keeps sampling while waiting for probes to
    /// return within tolerance (recovery) before giving up.
    pub recovery_timeout: Duration,
}

impl Default for SamplingConfig {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(1),
            max_during_samples: 300,
            recovery_timeout: Duration::from_secs(30),
        }
    }
}

/// Outcome of executing a single activity via a provider.
#[derive(Debug, Clone)]
pub struct ActivityOutcome {
    pub success: bool,
    pub output: Option<String>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// Trait for executing activities -- allows mocking in tests.
pub trait ActivityExecutor: Send + Sync {
    fn execute(&self, activity: &Activity) -> ActivityOutcome;
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
}

/// Configuration for an experiment run.
///
/// Dry-run and baseline-skip are handled at the CLI layer before
/// calling `run_experiment`, so they are not part of this config.
pub struct RunConfig {
    pub rollback_strategy: RollbackStrategy,
    /// Optional cancellation token. When cancelled, the runner stops before
    /// executing the next foreground activity, runs rollbacks for any fault
    /// already injected, and ends the run with `ExperimentStatus::Interrupted`
    /// — a cancelled run is never reported `Completed`.
    pub cancellation_token: Option<CancellationToken>,
    /// Optional parent OpenTelemetry context. When provided, the root
    /// `resilience.experiment` span is created as a child of this context,
    /// enabling cross-service trace linking (e.g. from an MCP tool span).
    pub parent_context: Option<opentelemetry::Context>,
    /// Optional load test executor. When provided and the experiment has
    /// a `load` config, the runner starts the load tool in the background
    /// during method execution.
    pub load_executor: Option<std::sync::Arc<dyn LoadExecutor>>,
    /// Optional override for the experiment's `max_concurrent_faults`
    /// blast-radius cap. When `Some`, it takes precedence over the value
    /// declared in the experiment; when `None`, the experiment's value (if
    /// any) is used.
    pub max_concurrent_faults: Option<u32>,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            rollback_strategy: RollbackStrategy::OnDeviation,
            cancellation_token: None,
            parent_context: None,
            load_executor: None,
            max_concurrent_faults: None,
        }
    }
}

#[cfg(test)]
mod tests;
