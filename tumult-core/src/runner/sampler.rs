//! During-phase probe sampler: a background thread that samples hypothesis
//! probes while the experiment method runs, so the during-phase result
//! reflects behaviour while the fault is active.

use std::sync::{mpsc, Arc, Mutex, PoisonError};

use crate::types::{DuringResult, Experiment};

use super::phases::{build_during_result, collect_during_samples, ProbeSampleMap};
use super::telemetry::epoch_nanos_now;
use super::{ActivityExecutor, SamplingConfig};

/// Handle to the background thread that samples hypothesis probes while the
/// method runs.
pub(super) struct DuringSampler {
    /// Dropped to signal the sampler thread to stop.
    stop_tx: mpsc::Sender<()>,
    /// Joins to the sampling end timestamp (epoch nanoseconds).
    handle: std::thread::JoinHandle<i64>,
    /// Shared sample sink; written incrementally so samples collected before
    /// a sampler panic are not lost.
    samples: Arc<Mutex<ProbeSampleMap>>,
    started_at_ns: i64,
}

/// Spawn the during-phase sampler thread, if the experiment has hypothesis
/// probes to sample. Probes are sampled on `sampling.interval` (up to
/// `sampling.max_during_samples` rounds) while the method executes, so the
/// during-phase result reflects behavior while the fault is active.
pub(super) fn spawn_during_sampler(
    experiment: &Experiment,
    executor: &Arc<dyn ActivityExecutor>,
    sampling: &SamplingConfig,
) -> Option<DuringSampler> {
    let hypothesis = experiment
        .steady_state_hypothesis
        .as_ref()
        .filter(|hypothesis| !hypothesis.probes.is_empty())?
        .clone();

    let executor = Arc::clone(executor);
    let samples = Arc::new(Mutex::new(ProbeSampleMap::new()));
    let sink = Arc::clone(&samples);
    let interval = sampling.interval;
    let max_samples = sampling.max_during_samples;
    let (stop_tx, stop_rx) = mpsc::channel();
    let started_at_ns = epoch_nanos_now();

    let handle = std::thread::spawn(move || {
        collect_during_samples(
            &hypothesis,
            executor.as_ref(),
            interval,
            max_samples,
            &stop_rx,
            &sink,
        );
        epoch_nanos_now()
    });

    Some(DuringSampler {
        stop_tx,
        handle,
        samples,
        started_at_ns,
    })
}

/// Stop the during-phase sampler and build its result. A panicked sampler
/// thread is logged and downgraded to "use whatever samples were collected"
/// rather than propagating the panic into the runner.
pub(super) fn finish_during_sampler(
    sampler: DuringSampler,
    sample_interval_s: f64,
) -> Option<DuringResult> {
    let DuringSampler {
        stop_tx,
        handle,
        samples,
        started_at_ns,
    } = sampler;

    // Dropping the sender disconnects the sampler's receiver, waking it
    // immediately from its inter-sample wait.
    drop(stop_tx);

    let ended_at_ns = match handle.join() {
        Ok(ns) => ns,
        Err(_panic) => {
            tracing::warn!(
                "during-phase probe sampling thread panicked; \
                 continuing with samples collected so far"
            );
            epoch_nanos_now()
        }
    };

    let collected: Vec<_> = samples
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drain()
        .collect();

    build_during_result(started_at_ns, ended_at_ns, sample_interval_s, &collected)
}
