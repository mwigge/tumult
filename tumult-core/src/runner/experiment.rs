//! `run_experiment` — the five-phase experiment orchestrator.

use std::sync::{mpsc, Arc, Mutex, PoisonError};
use std::time::Instant;

use crate::controls::{ControlRegistry, LifecycleEvent};
use crate::engine::determine_status;
use crate::execution::all_succeeded;
use crate::types::{
    ActivityStatus, DuringResult, Experiment, ExperimentStatus, HypothesisResult, Journal,
};

use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::KeyValue;

use super::activity::{evaluate_hypothesis, execute_activities, run_rollbacks};
use super::phases::{
    build_during_result, build_post_result, collect_during_samples, collect_post_samples,
    compute_analysis, ProbeSampleMap,
};
use super::telemetry::{epoch_nanos_now, make_interrupted_journal};
use super::{load, ActivityExecutor, RunConfig, RunnerError, SamplingConfig, TRACER_NAME};

/// Run an experiment through the five-phase lifecycle with the default
/// probe-sampling cadence (see [`SamplingConfig::default`]).
///
/// This is the main entry point for experiment execution. It takes an
/// experiment definition, an executor for running activities, a controls
/// registry for lifecycle hooks, and a run configuration.
///
/// Returns a Journal containing the complete experiment results.
///
/// # Errors
///
/// Returns [`RunnerError::EmptyMethod`] if the experiment has no method steps.
pub fn run_experiment(
    experiment: &Experiment,
    executor: &Arc<dyn ActivityExecutor>,
    controls: &Arc<ControlRegistry>,
    config: &RunConfig,
) -> Result<Journal, RunnerError> {
    run_experiment_with_sampling(
        experiment,
        executor,
        controls,
        config,
        &SamplingConfig::default(),
    )
}

/// Run an experiment through the five-phase lifecycle with an explicit
/// probe-sampling cadence.
///
/// Like [`run_experiment`], but `sampling` controls the during-phase probe
/// sampling interval and the post-phase recovery timeout.
///
/// # Errors
///
/// Returns [`RunnerError::EmptyMethod`] if the experiment has no method steps.
#[allow(clippy::too_many_lines)]
// run_experiment_with_sampling is a top-level orchestrator; splitting it
// further would harm readability.
pub fn run_experiment_with_sampling(
    experiment: &Experiment,
    executor: &Arc<dyn ActivityExecutor>,
    controls: &Arc<ControlRegistry>,
    config: &RunConfig,
    sampling: &SamplingConfig,
) -> Result<Journal, RunnerError> {
    if experiment.method.is_empty() {
        return Err(RunnerError::EmptyMethod);
    }

    // Check cancellation before starting
    if let Some(ref token) = config.cancellation_token {
        if token.is_cancelled() {
            let now = epoch_nanos_now();
            return Ok(make_interrupted_journal(experiment, now));
        }
    }

    let started = Instant::now();
    let started_at_ns = epoch_nanos_now();
    let experiment_id = uuid::Uuid::new_v4().to_string();

    // Structured audit log: experiment start.  Fields are consumed by SIEM
    // pipelines and audit tooling for compliance / change traceability.
    let audit_user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "unknown".to_string());
    tracing::info!(
        experiment_id = %experiment_id,
        experiment_title = %experiment.title,
        user = %audit_user,
        started_at_ns = started_at_ns,
        "experiment.started"
    );

    // -- Root span: resilience.experiment wraps the entire lifecycle.
    let tracer = opentelemetry::global::tracer(TRACER_NAME);
    let exp_span = {
        let builder = tracer
            .span_builder("resilience.experiment")
            .with_attributes(vec![
                KeyValue::new("resilience.experiment.title", experiment.title.clone()),
                KeyValue::new("resilience.experiment.id", experiment_id.clone()),
            ]);
        // If a parent context was provided (e.g. from an MCP tool span), use it
        // so the experiment span is linked into the caller's trace.
        if let Some(ref parent_cx) = config.parent_context {
            builder.start_with_context(&tracer, parent_cx)
        } else {
            builder.start(&tracer)
        }
    };
    let exp_cx = opentelemetry::Context::current_with_span(exp_span);
    let _exp_guard = exp_cx.attach();

    // -- Phase 0: Record Estimate
    controls.emit(&LifecycleEvent::BeforeExperiment);

    // -- Phase 1: Baseline (skipped if configured or no baseline config)
    // Baseline acquisition is handled externally; we record the estimate.

    // -- Hypothesis BEFORE
    let hypothesis_before = run_hypothesis_phase(
        experiment,
        executor.as_ref(),
        controls.as_ref(),
        "resilience.hypothesis.before",
    );

    let hypothesis_before_met = hypothesis_before.as_ref().map(|h| h.met);

    // If hypothesis before failed, abort -- skip method, go to rollbacks
    if hypothesis_before_met == Some(false) {
        let ended_at_ns = epoch_nanos_now();
        // Experiment durations never exceed u64::MAX milliseconds (~585M years).
        #[allow(clippy::cast_possible_truncation)]
        let duration_ms = started.elapsed().as_millis() as u64;

        // Run rollbacks if strategy says so and there are rollbacks to run
        let rollback_results = run_rollbacks(
            experiment,
            executor,
            controls,
            &config.rollback_strategy,
            true,
        );

        controls.emit(&LifecycleEvent::AfterExperiment);

        // Rollback failure counts in chaos experiments are always << u32::MAX.
        #[allow(clippy::cast_possible_truncation)]
        let rb_failures = rollback_results
            .iter()
            .filter(|r| r.status == ActivityStatus::Failed)
            .count() as u32;

        return Ok(Journal {
            ended_at_ns,
            duration_ms,
            steady_state_before: hypothesis_before,
            rollback_results,
            rollback_failures: rb_failures,
            ..Journal::for_experiment(
                experiment,
                experiment_id,
                ExperimentStatus::Aborted,
                started_at_ns,
            )
        });
    }

    // -- Check cancellation before method
    if let Some(ref token) = config.cancellation_token {
        if token.is_cancelled() {
            let ended_at_ns = epoch_nanos_now();
            // Experiment durations never exceed u64::MAX milliseconds (~585M years).
            #[allow(clippy::cast_possible_truncation)]
            let duration_ms = started.elapsed().as_millis() as u64;
            controls.emit(&LifecycleEvent::AfterExperiment);
            return Ok(Journal {
                ended_at_ns,
                duration_ms,
                steady_state_before: hypothesis_before,
                ..Journal::for_experiment(
                    experiment,
                    experiment_id,
                    ExperimentStatus::Interrupted,
                    started_at_ns,
                )
            });
        }
    }

    // -- Start load test (background, if configured)
    let (load_span_guard, load_handle) = load::start_load(experiment, config);

    // -- Phase 2: Execute Method (DURING)
    controls.emit(&LifecycleEvent::BeforeMethod);

    // Sample probes concurrently with method execution, on a separate
    // thread, so `during_result` reflects probe behavior while the fault
    // is actually active (rather than after the method, hypothesis-after,
    // and rollbacks have already completed).
    let during_sampler = spawn_during_sampler(experiment, executor, sampling);

    let method_results = execute_activities(
        &experiment.method,
        executor.as_ref(),
        controls.as_ref(),
        config.cancellation_token.as_ref(),
    );
    controls.emit(&LifecycleEvent::AfterMethod);

    let actions_succeeded = all_succeeded(&method_results);

    let during_result = during_sampler
        .and_then(|sampler| finish_during_sampler(sampler, sampling.interval.as_secs_f64()));

    // -- Phase 3: POST -- recovery measurement, taken immediately after the
    // method completes (and before hypothesis-after / rollback run), so
    // `post_result.recovery_time_s` reflects recovery from the fault itself
    // rather than from rollback actions. Probes are sampled on the
    // configured interval until they pass their tolerance (recovery) or the
    // recovery timeout elapses.
    let post_result = experiment
        .steady_state_hypothesis
        .as_ref()
        .filter(|hypothesis| !hypothesis.probes.is_empty())
        .and_then(|hypothesis| {
            let started_at_ns = epoch_nanos_now();
            let samples = collect_post_samples(
                hypothesis,
                executor.as_ref(),
                sampling.interval,
                sampling.recovery_timeout,
                config.cancellation_token.as_ref(),
            );
            let ended_at_ns = epoch_nanos_now();
            build_post_result(started_at_ns, ended_at_ns, &samples)
        });

    // -- Stop load test, collect results, and enrich the span
    let load_result = load::stop_load(load_handle, config.load_executor.as_ref());

    // Drop the load span guard so the span is exported
    drop(load_span_guard);

    // -- Hypothesis AFTER
    let hypothesis_after = run_hypothesis_phase(
        experiment,
        executor.as_ref(),
        controls.as_ref(),
        "resilience.hypothesis.after",
    );

    let hypothesis_after_met = hypothesis_after.as_ref().map(|h| h.met);

    // -- Determine status
    let status = determine_status(
        hypothesis_before_met,
        hypothesis_after_met,
        actions_succeeded,
    );

    // -- Rollbacks
    let deviated = status == ExperimentStatus::Deviated;
    let rollback_results = run_rollbacks(
        experiment,
        executor,
        controls,
        &config.rollback_strategy,
        deviated,
    );

    // -- Phase 4: Analysis
    let analysis = compute_analysis(experiment, &status);

    let ended_at_ns = epoch_nanos_now();
    // Experiment durations never exceed u64::MAX milliseconds (~585M years).
    #[allow(clippy::cast_possible_truncation)]
    let duration_ms = started.elapsed().as_millis() as u64;

    controls.emit(&LifecycleEvent::AfterExperiment);

    // Rollback failure counts in chaos experiments are always << u32::MAX.
    #[allow(clippy::cast_possible_truncation)]
    let rb_failures = rollback_results
        .iter()
        .filter(|r| r.status == ActivityStatus::Failed)
        .count() as u32;

    // Structured audit log: experiment completion.
    let deviations = u32::from(status == ExperimentStatus::Deviated);
    tracing::info!(
        experiment_id = %experiment_id,
        experiment_title = %experiment.title,
        status = ?status,
        duration_ms = duration_ms,
        deviations = deviations,
        "experiment.completed"
    );

    Ok(Journal {
        ended_at_ns,
        duration_ms,
        steady_state_before: hypothesis_before,
        steady_state_after: hypothesis_after,
        method_results,
        rollback_results,
        rollback_failures: rb_failures,
        during_result,
        post_result,
        load_result,
        analysis,
        ..Journal::for_experiment(experiment, experiment_id, status, started_at_ns)
    })
}

/// Evaluate the steady-state hypothesis (when present) inside a dedicated
/// span, bracketed by the `BeforeHypothesis`/`AfterHypothesis` lifecycle
/// events. `span_name` distinguishes the before/after phases.
fn run_hypothesis_phase(
    experiment: &Experiment,
    executor: &dyn ActivityExecutor,
    controls: &ControlRegistry,
    span_name: &'static str,
) -> Option<HypothesisResult> {
    let hypothesis = experiment.steady_state_hypothesis.as_ref()?;
    controls.emit(&LifecycleEvent::BeforeHypothesis);
    let hyp_tracer = opentelemetry::global::tracer(TRACER_NAME);
    let hyp_span = hyp_tracer
        .span_builder(span_name)
        .with_attributes(vec![KeyValue::new(
            "resilience.hypothesis.title",
            hypothesis.title.clone(),
        )])
        .start(&hyp_tracer);
    let hyp_cx = opentelemetry::Context::current_with_span(hyp_span);
    let _hyp_guard = hyp_cx.attach();
    let result = evaluate_hypothesis(hypothesis, executor, controls);
    controls.emit(&LifecycleEvent::AfterHypothesis);
    Some(result)
}

/// Handle to the background thread that samples hypothesis probes while the
/// method runs.
struct DuringSampler {
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
fn spawn_during_sampler(
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
fn finish_during_sampler(sampler: DuringSampler, sample_interval_s: f64) -> Option<DuringResult> {
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
