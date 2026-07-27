//! `run_experiment` — the five-phase experiment orchestrator.

use std::sync::Arc;
use std::time::Instant;

use tokio_util::sync::CancellationToken;

use crate::controls::{ControlRegistry, LifecycleEvent};
use crate::engine::determine_status;
use crate::execution::all_succeeded;
use crate::types::{
    ActivityStatus, ActivityType, BlastRadiusRecord, Experiment, ExperimentStatus,
    HypothesisResult, Journal,
};

use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::KeyValue;

use super::activity::{evaluate_hypothesis, execute_activities, run_rollbacks};
use super::guard::{finish_guard_monitor, spawn_guard_monitor};
use super::phases::{build_post_result, collect_post_samples, compute_analysis};
use super::sampler::{finish_during_sampler, spawn_during_sampler};
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
        record_experiment_outcome(&started, false);

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
            record_experiment_outcome(&started, false);
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

    // The method executes under a child of the caller's cancellation token so
    // that (a) the caller can still cancel the whole run, and (b) a breached
    // auto-halt guard can cancel the method mid-flight without disturbing the
    // caller's own token.
    let method_token = match &config.cancellation_token {
        Some(token) => token.child_token(),
        None => CancellationToken::new(),
    };

    // Sample probes concurrently with method execution, on a separate
    // thread, so `during_result` reflects probe behavior while the fault
    // is actually active (rather than after the method, hypothesis-after,
    // and rollbacks have already completed).
    let during_sampler = spawn_during_sampler(experiment, executor, sampling);

    // Auto-halt guards: a monitor thread evaluates each guard on the sampling
    // interval and cancels `method_token` the moment a guard breaches its
    // safe-condition tolerance `min_breaches` times in a row.
    let method_started = Instant::now();
    let guard_monitor = spawn_guard_monitor(
        experiment,
        executor,
        sampling,
        &method_token,
        method_started,
    );

    let effective_max_faults = config
        .max_concurrent_faults
        .or(experiment.max_concurrent_faults);

    let (method_results, peak_concurrent_faults) = execute_activities(
        &experiment.method,
        executor.as_ref(),
        controls.as_ref(),
        Some(&method_token),
        effective_max_faults,
    );
    controls.emit(&LifecycleEvent::AfterMethod);

    let actions_succeeded = all_succeeded(&method_results);

    let during_result = during_sampler
        .and_then(|sampler| finish_during_sampler(sampler, sampling.interval.as_secs_f64()));

    // Stop the guard monitor and learn whether a guard pulled the plug.
    let halt = guard_monitor.and_then(finish_guard_monitor);

    let blast_radius = build_blast_radius(
        experiment.blast_radius.clone(),
        effective_max_faults,
        peak_concurrent_faults,
    );

    // -- Auto-halt path: a guard breached mid-experiment. Skip post-phase and
    // hypothesis-after (we pulled the plug), stop load, run rollbacks
    // (treating the halt as needing rollback), and mark the run `Halted`.
    if let Some(mut halt) = halt {
        let load_result = load::stop_load(load_handle, config.load_executor.as_ref());
        drop(load_span_guard);

        let rollback_started = Instant::now();
        let rollback_results = run_rollbacks(
            experiment,
            executor,
            controls,
            &config.rollback_strategy,
            true,
        );
        // Rollback durations never exceed u64::MAX milliseconds.
        #[allow(clippy::cast_possible_truncation)]
        {
            halt.rollback_ms = rollback_started.elapsed().as_millis() as u64;
        }

        let status = ExperimentStatus::Halted;
        let analysis = compute_analysis(experiment, &status);

        let ended_at_ns = epoch_nanos_now();
        // Experiment durations never exceed u64::MAX milliseconds.
        #[allow(clippy::cast_possible_truncation)]
        let duration_ms = started.elapsed().as_millis() as u64;

        controls.emit(&LifecycleEvent::AfterExperiment);

        // Rollback failure counts in chaos experiments are always << u32::MAX.
        #[allow(clippy::cast_possible_truncation)]
        let rb_failures = rollback_results
            .iter()
            .filter(|r| r.status == ActivityStatus::Failed)
            .count() as u32;

        tracing::warn!(
            experiment_id = %experiment_id,
            experiment_title = %experiment.title,
            guard = %halt.guard_name,
            observed = ?halt.observed,
            safe_condition = %halt.safe_condition,
            time_to_halt_ms = halt.time_to_halt_ms,
            rollback_ms = halt.rollback_ms,
            "experiment.halted"
        );

        return Ok(Journal {
            ended_at_ns,
            duration_ms,
            steady_state_before: hypothesis_before,
            method_results,
            rollback_results,
            rollback_failures: rb_failures,
            during_result,
            load_result,
            analysis,
            halt: Some(halt),
            blast_radius,
            ..Journal::for_experiment(experiment, experiment_id, status, started_at_ns)
        });
    }

    // -- Mid-method cancellation: the caller's token fired while the method
    // ran (SIGINT broke the foreground loop). The run must never be reported
    // `Completed` from here on — stop load, run rollbacks (a fault may still
    // be injected), and mark it `Interrupted`.
    if method_token.is_cancelled() {
        let load_result = load::stop_load(load_handle, config.load_executor.as_ref());
        drop(load_span_guard);

        let rollback_results = run_rollbacks(
            experiment,
            executor,
            controls,
            &config.rollback_strategy,
            true,
        );

        let status = ExperimentStatus::Interrupted;
        let analysis = compute_analysis(experiment, &status);

        let ended_at_ns = epoch_nanos_now();
        // Experiment durations never exceed u64::MAX milliseconds.
        #[allow(clippy::cast_possible_truncation)]
        let duration_ms = started.elapsed().as_millis() as u64;

        controls.emit(&LifecycleEvent::AfterExperiment);

        // Rollback failure counts in chaos experiments are always << u32::MAX.
        #[allow(clippy::cast_possible_truncation)]
        let rb_failures = rollback_results
            .iter()
            .filter(|r| r.status == ActivityStatus::Failed)
            .count() as u32;

        tracing::warn!(
            experiment_id = %experiment_id,
            experiment_title = %experiment.title,
            duration_ms = duration_ms,
            "experiment.interrupted"
        );

        return Ok(Journal {
            ended_at_ns,
            duration_ms,
            steady_state_before: hypothesis_before,
            method_results,
            rollback_results,
            rollback_failures: rb_failures,
            during_result,
            load_result,
            analysis,
            blast_radius,
            ..Journal::for_experiment(experiment, experiment_id, status, started_at_ns)
        });
    }

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
    // Deviation calls for cleanup; so does a failure after a fault-injecting
    // (Action) activity actually started — the injected fault must be rolled
    // back even though the run "failed" rather than "deviated".
    let fault_started = method_results
        .iter()
        .any(|r| r.activity_type == ActivityType::Action);
    let needs_rollback = status == ExperimentStatus::Deviated
        || (status == ExperimentStatus::Failed && fault_started);
    let rollback_results = run_rollbacks(
        experiment,
        executor,
        controls,
        &config.rollback_strategy,
        needs_rollback,
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

    // Per-experiment metrics: outcome counter, lifecycle duration, and a
    // deviation count tagged with the experiment title when the steady
    // state was breached.
    record_experiment_outcome(&started, status == ExperimentStatus::Completed);
    if status == ExperimentStatus::Deviated {
        tumult_otel::instrument::record_deviation(
            tumult_otel::TumultMetrics::global(),
            &experiment.title,
        );
    }

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
        blast_radius,
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

/// Build the blast-radius journal record, or `None` when there is nothing to
/// report (no note, no cap, and no background faults ran).
fn build_blast_radius(
    note: Option<String>,
    max_concurrent_faults: Option<u32>,
    peak_concurrent_faults: u32,
) -> Option<BlastRadiusRecord> {
    if note.is_none() && max_concurrent_faults.is_none() && peak_concurrent_faults == 0 {
        return None;
    }
    Some(BlastRadiusRecord {
        note,
        max_concurrent_faults,
        peak_concurrent_faults,
    })
}

/// Record the outcome of a finished experiment: the `tumult.experiments.total`
/// counter and the `tumult.experiment.duration` histogram, both tagged with
/// the outcome. No-op when no meter provider is installed.
fn record_experiment_outcome(started: &Instant, success: bool) {
    let metrics = tumult_otel::TumultMetrics::global();
    tumult_otel::instrument::record_experiment(metrics, success);
    tumult_otel::instrument::record_experiment_duration(
        metrics,
        started.elapsed().as_secs_f64(),
        success,
    );
}
