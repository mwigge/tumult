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

use std::time::Instant;

use crate::controls::{ControlRegistry, LifecycleEvent};
use crate::engine::{determine_status, evaluate_tolerance};
use crate::execution::{
    all_succeeded, make_result, partition_background, should_rollback, ResultParams,
    RollbackStrategy,
};
use crate::types::{
    Activity, ActivityResult, ActivityStatus, ActivityType, AnalysisResult, DuringResult,
    ExpectedOutcome, Experiment, ExperimentStatus, GameDay, GameDayJournal, Hypothesis,
    HypothesisResult, Journal, LoadConfig, LoadResult, PostResult, ProbeDuring, ProbePost,
    Provider, ResilienceScore, SpanId, TraceId,
};

use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::KeyValue;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

const TRACER_NAME: &str = "tumult-engine";

#[derive(Error, Debug)]
pub enum RunnerError {
    #[error("experiment has no method steps")]
    EmptyMethod,
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
    /// Optional cancellation token. When cancelled, the runner returns
    /// `ExperimentStatus::Interrupted` before executing the next activity.
    pub cancellation_token: Option<CancellationToken>,
    /// Optional parent OpenTelemetry context. When provided, the root
    /// `resilience.experiment` span is created as a child of this context,
    /// enabling cross-service trace linking (e.g. from an MCP tool span).
    pub parent_context: Option<opentelemetry::Context>,
    /// Optional load test executor. When provided and the experiment has
    /// a `load` config, the runner starts the load tool in the background
    /// during method execution.
    pub load_executor: Option<std::sync::Arc<dyn LoadExecutor>>,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            rollback_strategy: RollbackStrategy::OnDeviation,
            cancellation_token: None,
            parent_context: None,
            load_executor: None,
        }
    }
}

/// Run an experiment through the five-phase lifecycle.
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
#[allow(clippy::too_many_lines)]
// run_experiment is a top-level orchestrator; splitting it further would harm readability.
pub fn run_experiment(
    experiment: &Experiment,
    executor: &std::sync::Arc<dyn ActivityExecutor>,
    controls: &std::sync::Arc<ControlRegistry>,
    config: &RunConfig,
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
    let hypothesis_before = if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        controls.emit(&LifecycleEvent::BeforeHypothesis);
        let hyp_tracer = opentelemetry::global::tracer(TRACER_NAME);
        let hyp_span = hyp_tracer
            .span_builder("resilience.hypothesis.before")
            .with_attributes(vec![KeyValue::new(
                "resilience.hypothesis.title",
                hypothesis.title.clone(),
            )])
            .start(&hyp_tracer);
        let hyp_cx = opentelemetry::Context::current_with_span(hyp_span);
        let _hyp_guard = hyp_cx.attach();
        let result = evaluate_hypothesis(hypothesis, executor.as_ref(), controls.as_ref());
        controls.emit(&LifecycleEvent::AfterHypothesis);
        Some(result)
    } else {
        None
    };

    let hypothesis_before_met = hypothesis_before.as_ref().map(|h| h.met);

    // If hypothesis before failed, abort -- skip method, go to rollbacks
    if hypothesis_before_met == Some(false) {
        let ended_at_ns = epoch_nanos_now();
        // Experiment durations never exceed u64::MAX milliseconds (~585M years).
        #[allow(clippy::cast_possible_truncation)]
        let duration_ms = started.elapsed().as_millis() as u64;

        // Run rollbacks if strategy says so and there are rollbacks to run
        let rollback_results = if !experiment.rollbacks.is_empty()
            && should_rollback(&config.rollback_strategy, true)
        {
            controls.emit(&LifecycleEvent::BeforeRollback);
            let rb_tracer = opentelemetry::global::tracer(TRACER_NAME);
            let rb_span = rb_tracer
                .span_builder("resilience.rollback")
                .start(&rb_tracer);
            let rb_cx = opentelemetry::Context::current_with_span(rb_span);
            let _rb_guard = rb_cx.attach();
            let results = execute_rollback_activities(
                &experiment.rollbacks,
                executor.as_ref(),
                controls.as_ref(),
            );
            controls.emit(&LifecycleEvent::AfterRollback);
            results
        } else {
            vec![]
        };

        controls.emit(&LifecycleEvent::AfterExperiment);

        // Rollback failure counts in chaos experiments are always << u32::MAX.
        #[allow(clippy::cast_possible_truncation)]
        let rb_failures = rollback_results
            .iter()
            .filter(|r| r.status == ActivityStatus::Failed)
            .count() as u32;

        return Ok(Journal {
            experiment_title: experiment.title.clone(),
            experiment_id,
            status: ExperimentStatus::Aborted,
            started_at_ns,
            ended_at_ns,
            duration_ms,
            steady_state_before: hypothesis_before,
            steady_state_after: None,
            method_results: vec![],
            rollback_results,
            rollback_failures: rb_failures,
            estimate: experiment.estimate.clone(),
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: experiment.regulatory.clone(),
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
                experiment_title: experiment.title.clone(),
                experiment_id,
                status: ExperimentStatus::Interrupted,
                started_at_ns,
                ended_at_ns,
                duration_ms,
                steady_state_before: hypothesis_before,
                steady_state_after: None,
                method_results: vec![],
                rollback_results: vec![],
                rollback_failures: 0,
                estimate: experiment.estimate.clone(),
                baseline_result: None,
                during_result: None,
                post_result: None,
                load_result: None,
                analysis: None,
                regulatory: experiment.regulatory.clone(),
            });
        }
    }

    // -- Start load test (background, if configured)
    let load_tracer = opentelemetry::global::tracer(TRACER_NAME);
    let load_span_guard = if let Some(ref load_config) = experiment.load {
        let tool_name = format!("{}", load_config.tool);
        let span = load_tracer
            .span_builder("resilience.load")
            .with_attributes(vec![
                KeyValue::new("resilience.load.tool", tool_name),
                KeyValue::new(
                    "resilience.load.vus",
                    i64::from(load_config.vus.unwrap_or(0)),
                ),
                KeyValue::new(
                    "resilience.load.script",
                    load_config.script.display().to_string(),
                ),
            ])
            .start(&load_tracer);
        let cx = opentelemetry::Context::current_with_span(span);
        Some(cx.attach())
    } else {
        None
    };

    let load_handle = if let (Some(ref load_config), Some(ref load_exec)) =
        (&experiment.load, &config.load_executor)
    {
        match load_exec.start(load_config) {
            Ok(handle) => {
                tracing::info!(
                    tool = %load_config.tool,
                    script = %load_config.script.display(),
                    "load test started"
                );
                Some(handle)
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to start load test");
                None
            }
        }
    } else {
        None
    };

    // -- Phase 2: Execute Method (DURING)
    controls.emit(&LifecycleEvent::BeforeMethod);
    let method_results = execute_activities(
        &experiment.method,
        executor.as_ref(),
        controls.as_ref(),
        config.cancellation_token.as_ref(),
    );
    controls.emit(&LifecycleEvent::AfterMethod);

    let actions_succeeded = all_succeeded(&method_results);

    // -- Stop load test, collect results, and enrich the span
    let load_result =
        if let (Some(handle), Some(ref load_exec)) = (load_handle, &config.load_executor) {
            match load_exec.stop(handle) {
                Ok(result) => {
                    // Enrich the resilience.load span with result metrics
                    let span_cx = opentelemetry::Context::current();
                    let span = span_cx.span();
                    span.set_attribute(KeyValue::new(
                        "resilience.load.throughput_rps",
                        result.throughput_rps,
                    ));
                    span.set_attribute(KeyValue::new(
                        "resilience.load.latency_p50_ms",
                        result.latency_p50_ms,
                    ));
                    span.set_attribute(KeyValue::new(
                        "resilience.load.latency_p95_ms",
                        result.latency_p95_ms,
                    ));
                    span.set_attribute(KeyValue::new(
                        "resilience.load.latency_p99_ms",
                        result.latency_p99_ms,
                    ));
                    span.set_attribute(KeyValue::new(
                        "resilience.load.error_rate",
                        result.error_rate,
                    ));
                    span.set_attribute(KeyValue::new(
                        "resilience.load.total_requests",
                        i64::try_from(result.total_requests).unwrap_or(i64::MAX),
                    ));
                    span.set_attribute(KeyValue::new(
                        "resilience.load.thresholds_met",
                        result.thresholds_met,
                    ));
                    span.set_attribute(KeyValue::new(
                        "resilience.load.duration_s",
                        result.duration_s,
                    ));

                    tracing::info!(
                        throughput_rps = result.throughput_rps,
                        latency_p95_ms = result.latency_p95_ms,
                        error_rate = result.error_rate,
                        "load test completed"
                    );
                    Some(result)
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to collect load test results");
                    None
                }
            }
        } else {
            None
        };

    // Drop the load span guard so the span is exported
    drop(load_span_guard);

    // -- Phase 3: POST -- recovery measurement
    // Post-phase sampling is done externally; hypothesis after captures it.

    // -- Hypothesis AFTER
    let hypothesis_after = if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        controls.emit(&LifecycleEvent::BeforeHypothesis);
        let hyp_tracer = opentelemetry::global::tracer(TRACER_NAME);
        let hyp_span = hyp_tracer
            .span_builder("resilience.hypothesis.after")
            .with_attributes(vec![KeyValue::new(
                "resilience.hypothesis.title",
                hypothesis.title.clone(),
            )])
            .start(&hyp_tracer);
        let hyp_cx = opentelemetry::Context::current_with_span(hyp_span);
        let _hyp_guard = hyp_cx.attach();
        let result = evaluate_hypothesis(hypothesis, executor.as_ref(), controls.as_ref());
        controls.emit(&LifecycleEvent::AfterHypothesis);
        Some(result)
    } else {
        None
    };

    let hypothesis_after_met = hypothesis_after.as_ref().map(|h| h.met);

    // -- Determine status
    let status = determine_status(
        hypothesis_before_met,
        hypothesis_after_met,
        actions_succeeded,
    );

    // -- Rollbacks
    let deviated = status == ExperimentStatus::Deviated;
    let rollback_results = if !experiment.rollbacks.is_empty()
        && should_rollback(&config.rollback_strategy, deviated)
    {
        controls.emit(&LifecycleEvent::BeforeRollback);
        let rb_tracer = opentelemetry::global::tracer(TRACER_NAME);
        let rb_span = rb_tracer
            .span_builder("resilience.rollback")
            .start(&rb_tracer);
        let rb_cx = opentelemetry::Context::current_with_span(rb_span);
        let _rb_guard = rb_cx.attach();
        let results = execute_rollback_activities(
            &experiment.rollbacks,
            executor.as_ref(),
            controls.as_ref(),
        );
        controls.emit(&LifecycleEvent::AfterRollback);
        results
    } else {
        vec![]
    };

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

    // -- During-phase and post-phase probe sampling
    let (during_result, post_result) =
        if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
            // During-phase: sample probes to capture behavior while fault is active
            let during_start = epoch_nanos_now();
            let during_samples = collect_probe_samples(hypothesis, executor.as_ref(), 3);
            let during_end = epoch_nanos_now();
            let during = build_during_result(during_start, during_end, &during_samples);

            // Post-phase: sample probes to measure recovery after method completion
            let post_start = epoch_nanos_now();
            let post_samples = collect_probe_samples(hypothesis, executor.as_ref(), 3);
            let post_end = epoch_nanos_now();
            let post = build_post_result(post_start, post_end, &post_samples);

            (during, post)
        } else {
            (None, None)
        };

    Ok(Journal {
        experiment_title: experiment.title.clone(),
        experiment_id,
        status,
        started_at_ns,
        ended_at_ns,
        duration_ms,
        steady_state_before: hypothesis_before,
        steady_state_after: hypothesis_after,
        method_results,
        rollback_results,
        rollback_failures: rb_failures,
        estimate: experiment.estimate.clone(),
        baseline_result: None,
        during_result,
        post_result,
        load_result,
        analysis,
        regulatory: experiment.regulatory.clone(),
    })
}

/// Runs a `GameDay` — a coordinated campaign of experiments under shared load.
///
/// Iterates the provided experiments in sequence, optionally running a
/// shared load generator across all of them. Computes an aggregate
/// `ResilienceScore` and returns a `GameDayJournal`.
///
/// # Errors
///
/// Returns [`RunnerError`] if any experiment fails to execute (not if it
/// deviates — deviation is a valid outcome captured in the journal).
#[must_use = "the GameDayJournal contains the aggregate results"]
#[allow(clippy::too_many_lines)] // Orchestration function with OTel setup, load management, and scoring
pub fn run_gameday(
    gameday: &GameDay,
    experiments: &[Experiment],
    executor: &std::sync::Arc<dyn ActivityExecutor>,
    controls: &std::sync::Arc<ControlRegistry>,
    config: &RunConfig,
) -> Result<GameDayJournal, RunnerError> {
    let gameday_id = uuid::Uuid::new_v4().to_string();
    let started = std::time::Instant::now();
    let started_at_ns = epoch_nanos_now();

    // Create root GameDay OTel span
    let tracer = opentelemetry::global::tracer(TRACER_NAME);
    let gd_span = tracer
        .span_builder("resilience.gameday")
        .with_attributes(vec![
            KeyValue::new("resilience.gameday.id", gameday_id.clone()),
            KeyValue::new("resilience.gameday.title", gameday.title.clone()),
            KeyValue::new(
                "resilience.gameday.experiment_count",
                i64::try_from(experiments.len()).unwrap_or(0),
            ),
        ])
        .start(&tracer);
    let gd_cx = opentelemetry::Context::current_with_span(gd_span);
    let _gd_guard = gd_cx.attach();

    // Start shared load (if configured)
    let load_handle = if let (Some(ref load_config), Some(ref load_exec)) =
        (&gameday.load, &config.load_executor)
    {
        match load_exec.start(load_config) {
            Ok(handle) => {
                tracing::info!(
                    tool = %load_config.tool,
                    "gameday load started"
                );
                Some(handle)
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to start gameday load");
                None
            }
        }
    } else {
        None
    };

    // Run each experiment with the GameDay span as parent context
    let mut journals = Vec::with_capacity(experiments.len());
    for experiment in experiments {
        let exp_config = RunConfig {
            rollback_strategy: config.rollback_strategy.clone(),
            cancellation_token: config.cancellation_token.clone(),
            parent_context: Some(opentelemetry::Context::current()),
            load_executor: None, // load is managed at GameDay level
        };
        match run_experiment(experiment, executor, controls, &exp_config) {
            Ok(journal) => journals.push(journal),
            Err(e) => {
                tracing::error!(error = %e, title = %experiment.title, "gameday experiment failed");
                return Err(e);
            }
        }
    }

    // Stop load and collect results
    let load_result =
        if let (Some(handle), Some(ref load_exec)) = (load_handle, &config.load_executor) {
            match load_exec.stop(handle) {
                Ok(result) => {
                    tracing::info!(
                        throughput_rps = result.throughput_rps,
                        "gameday load completed"
                    );
                    Some(result)
                }
                Err(e) => {
                    tracing::error!(error = %e, "failed to collect gameday load results");
                    None
                }
            }
        } else {
            None
        };

    // Compute resilience score
    let total = journals.len();
    let passed = journals
        .iter()
        .filter(|j| j.status == ExperimentStatus::Completed)
        .count();
    #[allow(clippy::cast_precision_loss)]
    let pass_rate = if total > 0 {
        passed as f64 / total as f64
    } else {
        0.0
    };

    // Recovery compliance: check MTTR against target
    let recovery = compute_recovery_compliance(&journals, gameday.scoring.mttr_target_s);

    // Load impact tolerance (1.0 if no load, otherwise based on error rate)
    let load_impact = load_result
        .as_ref()
        .map_or(1.0, |lr| (1.0 - lr.error_rate).max(0.0));

    // Compliance coverage: count mapped articles that have passing experiments
    let compliance = compute_compliance_coverage(gameday, &journals);

    let score = ResilienceScore::compute(pass_rate, recovery, load_impact, compliance);
    let compliance_status = score.status().to_string();

    let ended_at_ns = epoch_nanos_now();
    #[allow(clippy::cast_precision_loss)]
    let duration_s = started.elapsed().as_secs_f64();

    Ok(GameDayJournal {
        gameday_id,
        title: gameday.title.clone(),
        started_at_ns,
        ended_at_ns,
        duration_s,
        experiment_journals: journals,
        load_result,
        resilience_score: score,
        compliance_status,
        regulatory: gameday.regulatory.clone(),
    })
}

/// Computes recovery compliance score from MTTR data in journals.
fn compute_recovery_compliance(journals: &[Journal], mttr_target_s: f64) -> f64 {
    let mut total_recovery = 0;
    let mut compliant_recovery = 0;

    for journal in journals {
        if let Some(ref post) = journal.post_result {
            total_recovery += 1;
            if post.recovery_time_s <= mttr_target_s && post.full_recovery {
                compliant_recovery += 1;
            }
        }
    }

    if total_recovery == 0 {
        1.0 // No recovery data → assume compliant
    } else {
        #[allow(clippy::cast_precision_loss)]
        {
            f64::from(compliant_recovery) / f64::from(total_recovery)
        }
    }
}

/// Computes compliance coverage from article mappings.
fn compute_compliance_coverage(gameday: &GameDay, journals: &[Journal]) -> f64 {
    // Collect all unique mapped articles
    let all_articles: std::collections::HashSet<&str> = gameday
        .experiments
        .iter()
        .flat_map(|e| e.compliance_maps.iter().map(String::as_str))
        .collect();

    if all_articles.is_empty() {
        return 1.0; // No articles mapped → full coverage by default
    }

    // An article is "met" if at least one experiment mapped to it completed
    let mut met = 0;
    for article in &all_articles {
        let has_passing = gameday.experiments.iter().enumerate().any(|(i, exp)| {
            exp.compliance_maps.iter().any(|a| a == article)
                && journals
                    .get(i)
                    .is_some_and(|j| j.status == ExperimentStatus::Completed)
        });
        if has_passing {
            met += 1;
        }
    }

    #[allow(clippy::cast_precision_loss)]
    {
        f64::from(met) / all_articles.len() as f64
    }
}

/// Evaluate a steady-state hypothesis by running its probes.
fn evaluate_hypothesis(
    hypothesis: &Hypothesis,
    executor: &dyn ActivityExecutor,
    controls: &ControlRegistry,
) -> HypothesisResult {
    let mut probe_results = Vec::with_capacity(hypothesis.probes.len());
    let mut all_met = true;

    let tracer = opentelemetry::global::tracer(TRACER_NAME);

    for probe in &hypothesis.probes {
        controls.emit(&LifecycleEvent::BeforeActivity {
            name: probe.name.clone(),
        });

        // Create an OTel span for this probe with target + fault attributes
        let mut attrs = vec![KeyValue::new("resilience.probe.name", probe.name.clone())];
        attrs.extend(target_attributes(probe));
        attrs.extend(fault_attributes(probe));
        let span = tracer
            .span_builder("resilience.probe".to_string())
            .with_attributes(attrs)
            .start(&tracer);
        let cx = opentelemetry::Context::current_with_span(span);
        let _guard = cx.attach();

        let started_at_ns = epoch_nanos_now();
        let outcome = executor.execute(probe);
        set_span_status_from_outcome(outcome.success, outcome.error.as_deref());

        let result = make_result(ResultParams {
            activity: probe,
            started_at_ns,
            duration_ms: outcome.duration_ms,
            success: outcome.success,
            output: outcome.output.clone(),
            error: outcome.error.clone(),
            trace_id: current_trace_id(),
            span_id: current_span_id(),
        });

        // Check tolerance if defined
        if let Some(ref tolerance) = probe.tolerance {
            if let Some(ref output) = outcome.output {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(output) {
                    if !evaluate_tolerance(&value, tolerance) {
                        all_met = false;
                    }
                } else {
                    // If output isn't valid JSON, try as string
                    let value = serde_json::Value::String(output.clone());
                    if !evaluate_tolerance(&value, tolerance) {
                        all_met = false;
                    }
                }
            } else {
                // Tolerance defined but no output -- cannot evaluate, treat as failure
                all_met = false;
            }
        } else if !outcome.success {
            all_met = false;
        }

        controls.emit(&LifecycleEvent::AfterActivity {
            name: probe.name.clone(),
        });

        probe_results.push(result);
    }

    HypothesisResult {
        title: hypothesis.title.clone(),
        met: all_met,
        probe_results,
    }
}

/// Execute a single activity with `OTel` instrumentation.
///
/// Extracted so both foreground and background paths share the same logic.
fn execute_single_activity(
    activity: &Activity,
    executor: &dyn ActivityExecutor,
    controls: &ControlRegistry,
) -> ActivityResult {
    let tracer = opentelemetry::global::tracer(TRACER_NAME);

    controls.emit(&LifecycleEvent::BeforeActivity {
        name: activity.name.clone(),
    });

    let span_name = match activity.activity_type {
        ActivityType::Action => "resilience.action",
        ActivityType::Probe => "resilience.probe",
    };
    let mut attrs = vec![
        KeyValue::new("resilience.action.name", activity.name.clone()),
        KeyValue::new(
            "resilience.activity.type",
            activity.activity_type.to_string(),
        ),
    ];
    attrs.extend(target_attributes(activity));
    attrs.extend(fault_attributes(activity));
    let span = tracer
        .span_builder(span_name.to_string())
        .with_attributes(attrs)
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    let _guard = cx.attach();

    let started_at_ns = epoch_nanos_now();
    let outcome = executor.execute(activity);
    set_span_status_from_outcome(outcome.success, outcome.error.as_deref());

    let result = make_result(ResultParams {
        activity,
        started_at_ns,
        duration_ms: outcome.duration_ms,
        success: outcome.success,
        output: outcome.output,
        error: outcome.error,
        trace_id: current_trace_id(),
        span_id: current_span_id(),
    });

    controls.emit(&LifecycleEvent::AfterActivity {
        name: activity.name.clone(),
    });

    result
}

/// Execute a list of activities, partitioning into foreground (sequential)
/// and background (spawned concurrently via `JoinSet`).
///
/// Foreground activities execute sequentially with pause handling.
/// Background activities are spawned immediately and joined after all
/// foreground work completes.
///
/// If a cancellation token is provided and cancelled, stops executing
/// remaining foreground activities and returns results collected so far
/// (background tasks are still joined).
fn execute_activities(
    activities: &[Activity],
    executor: &(dyn ActivityExecutor + Sync),
    controls: &ControlRegistry,
    cancellation_token: Option<&CancellationToken>,
) -> Vec<ActivityResult> {
    let (foreground, background) = partition_background(activities);

    // Capacity: foreground results first, then background joined at end.
    let mut fg_results = Vec::with_capacity(foreground.len());

    // Spawn background activities on scoped OS threads *then* run foreground
    // sequentially inside the same scope.  `std::thread::scope` guarantees all
    // background threads are joined before the scope exits (i.e. after foreground
    // completes), giving us true concurrency without unsafe lifetime extension.
    let bg_results: Vec<std::result::Result<ActivityResult, _>> = std::thread::scope(|scope| {
        // 1. Spawn background threads immediately.
        let handles: Vec<_> = background
            .iter()
            .map(|&activity| {
                scope.spawn(move || execute_single_activity(activity, executor, controls))
            })
            .collect();

        // 2. Run foreground activities sequentially while background threads run.
        //    Note: pause_before_s / pause_after_s use std::thread::sleep here
        //    because we are inside a synchronous scope closure.  Background
        //    threads are already running concurrently so blocking the OS thread
        //    here is acceptable.
        for &activity in &foreground {
            // Check cancellation before each activity.
            if let Some(token) = cancellation_token {
                if token.is_cancelled() {
                    tracing::warn!(
                        activity = %activity.name,
                        "cancelled before activity execution"
                    );
                    break;
                }
            }

            if let Some(pause) = activity.pause_before_s {
                if pause > 0.0 {
                    opentelemetry::Context::current().span().add_event(
                        "experiment.pause.before",
                        vec![
                            KeyValue::new("activity.name", activity.name.clone()),
                            KeyValue::new("pause_seconds", pause),
                        ],
                    );
                    std::thread::sleep(std::time::Duration::from_secs_f64(pause));
                    opentelemetry::Context::current().span().add_event(
                        "experiment.resume.before",
                        vec![KeyValue::new("activity.name", activity.name.clone())],
                    );
                }
            }

            let result = execute_single_activity(activity, executor, controls);

            if let Some(pause) = activity.pause_after_s {
                if pause > 0.0 {
                    opentelemetry::Context::current().span().add_event(
                        "experiment.pause.after",
                        vec![
                            KeyValue::new("activity.name", activity.name.clone()),
                            KeyValue::new("pause_seconds", pause),
                        ],
                    );
                    std::thread::sleep(std::time::Duration::from_secs_f64(pause));
                    opentelemetry::Context::current().span().add_event(
                        "experiment.resume.after",
                        vec![KeyValue::new("activity.name", activity.name.clone())],
                    );
                }
            }

            fg_results.push(result);
        }

        // 3. Join background threads (scope exit would also do this, but collect
        //    the results explicitly so we can handle panics below).
        handles
            .into_iter()
            .map(std::thread::ScopedJoinHandle::join)
            .collect()
    });

    // Foreground results first, then background -- preserving the expected ordering
    // (foreground is the "primary" execution path; background runs alongside it).
    let mut results = fg_results;
    results.reserve(background.len());

    for join_result in bg_results {
        match join_result {
            Ok(activity_result) => results.push(activity_result),
            Err(_panic) => {
                tracing::error!("background activity panicked");
                results.push(ActivityResult {
                    name: "background-task".into(),
                    activity_type: ActivityType::Action,
                    status: ActivityStatus::Failed,
                    started_at_ns: epoch_nanos_now(),
                    duration_ms: 0,
                    output: None,
                    error: Some("background activity panicked".to_string()),
                    trace_id: TraceId::empty(),
                    span_id: SpanId::empty(),
                });
            }
        }
    }

    results
}

/// Execute rollback activities. Unlike `execute_activities`, this function
/// continues executing remaining rollbacks even if one fails, logging a
/// warning for each failure.
fn execute_rollback_activities(
    activities: &[Activity],
    executor: &dyn ActivityExecutor,
    controls: &ControlRegistry,
) -> Vec<ActivityResult> {
    let mut results = Vec::with_capacity(activities.len());

    let tracer = opentelemetry::global::tracer(TRACER_NAME);

    for activity in activities {
        controls.emit(&LifecycleEvent::BeforeActivity {
            name: activity.name.clone(),
        });

        let span_name = match activity.activity_type {
            ActivityType::Action => "resilience.action",
            ActivityType::Probe => "resilience.probe",
        };
        let mut attrs = vec![
            KeyValue::new("resilience.action.name", activity.name.clone()),
            KeyValue::new(
                "resilience.activity.type",
                activity.activity_type.to_string(),
            ),
        ];
        attrs.extend(target_attributes(activity));
        attrs.extend(fault_attributes(activity));
        let span = tracer
            .span_builder(span_name.to_string())
            .with_attributes(attrs)
            .start(&tracer);
        let cx = opentelemetry::Context::current_with_span(span);
        let _guard = cx.attach();

        let started_at_ns = epoch_nanos_now();
        let outcome = executor.execute(activity);
        set_span_status_from_outcome(outcome.success, outcome.error.as_deref());

        if !outcome.success {
            tracing::warn!(
                activity = %activity.name,
                error = ?outcome.error,
                "rollback activity failed, continuing with remaining rollbacks"
            );
        }

        let result = make_result(ResultParams {
            activity,
            started_at_ns,
            duration_ms: outcome.duration_ms,
            success: outcome.success,
            output: outcome.output,
            error: outcome.error,
            trace_id: current_trace_id(),
            span_id: current_span_id(),
        });

        controls.emit(&LifecycleEvent::AfterActivity {
            name: activity.name.clone(),
        });

        results.push(result);
    }

    results
}

/// Compute Phase 4 analysis from estimate and actual results.
fn compute_analysis(experiment: &Experiment, status: &ExperimentStatus) -> Option<AnalysisResult> {
    let estimate = experiment.estimate.as_ref()?;

    // Compare estimate vs actual outcome
    let actual_recovered = *status == ExperimentStatus::Completed;
    let estimated_recovered = estimate.expected_outcome == ExpectedOutcome::Recovered;
    let estimate_accuracy = if actual_recovered == estimated_recovered {
        Some(1.0)
    } else {
        Some(0.0)
    };

    Some(AnalysisResult {
        estimate_accuracy,
        estimate_recovery_delta_s: None,
        trend: None,
        resilience_score: if actual_recovered {
            Some(1.0)
        } else {
            Some(0.0)
        },
    })
}

/// Run hypothesis probes a fixed number of times and return per-probe
/// sample results. Used for during-phase and post-phase collection.
fn collect_probe_samples(
    hypothesis: &Hypothesis,
    executor: &dyn ActivityExecutor,
    count: usize,
) -> Vec<(String, Vec<ActivityResult>)> {
    let mut per_probe: std::collections::HashMap<String, Vec<ActivityResult>> =
        std::collections::HashMap::new();

    for _ in 0..count {
        for probe in &hypothesis.probes {
            let start = Instant::now();
            let started_at_ns = epoch_nanos_now();
            let outcome = executor.execute(probe);
            // Probe durations never exceed u64::MAX milliseconds (~585M years).
            #[allow(clippy::cast_possible_truncation)]
            let elapsed = start.elapsed().as_millis() as u64;

            let status = if outcome.success {
                ActivityStatus::Succeeded
            } else {
                ActivityStatus::Failed
            };

            per_probe
                .entry(probe.name.clone())
                .or_default()
                .push(ActivityResult {
                    name: probe.name.clone(),
                    activity_type: ActivityType::Probe,
                    status,
                    started_at_ns,
                    duration_ms: elapsed,
                    output: outcome.output,
                    error: outcome.error,
                    trace_id: TraceId::empty(),
                    span_id: SpanId::empty(),
                });
        }
    }

    per_probe.into_iter().collect()
}

/// Build a `DuringResult` from probe samples collected while fault injection
/// was active. Returns `None` if no samples were collected.
fn build_during_result(
    started_at_ns: i64,
    ended_at_ns: i64,
    probe_samples: &[(String, Vec<ActivityResult>)],
) -> Option<DuringResult> {
    if probe_samples.is_empty() {
        return None;
    }

    // Nanosecond delta converted to seconds; i64 → f64 precision loss is
    // acceptable for human-readable fault duration display.
    #[allow(clippy::cast_precision_loss)]
    let fault_active_s = (ended_at_ns - started_at_ns) as f64 / 1_000_000_000.0;

    let probes: Vec<ProbeDuring> = probe_samples
        .iter()
        .map(|(name, samples)| {
            // Sample counts in chaos experiments are always << u32::MAX.
            #[allow(clippy::cast_possible_truncation)]
            let total = samples.len() as u32;
            // Sample counts in chaos experiments are always << u32::MAX.
            #[allow(clippy::cast_possible_truncation)]
            let failed = samples
                .iter()
                .filter(|s| s.status == ActivityStatus::Failed)
                .count() as u32;
            // u64 → f64 precision loss is acceptable for millisecond statistics display.
            #[allow(clippy::cast_precision_loss)]
            let durations: Vec<f64> = samples.iter().map(|s| s.duration_ms as f64).collect();
            // usize → f64 precision loss is acceptable for mean calculation with small N.
            #[allow(clippy::cast_precision_loss)]
            let mean = if durations.is_empty() {
                0.0
            } else {
                durations.iter().sum::<f64>() / durations.len() as f64
            };
            let max = durations.iter().copied().fold(f64::NAN, f64::max);
            let min = durations.iter().copied().fold(f64::NAN, f64::min);
            let breached_at_ns = samples
                .iter()
                .find(|s| s.status == ActivityStatus::Failed)
                .map(|s| s.started_at_ns);

            ProbeDuring {
                name: name.clone(),
                samples: total,
                mean,
                max,
                min,
                error_rate: if total > 0 {
                    f64::from(failed) / f64::from(total)
                } else {
                    0.0
                },
                breached_at_ns,
                breach_count: failed,
            }
        })
        .collect();

    Some(DuringResult {
        started_at_ns,
        ended_at_ns,
        fault_active_s,
        sample_interval_s: 1.0,
        probes,
        degradation_onset_s: None,
        degradation_peak_s: None,
        degradation_magnitude: None,
        graceful_degradation: None,
    })
}

/// Build a `PostResult` from probe samples collected after method completion
/// to measure system recovery. Returns `None` if no samples were collected.
fn build_post_result(
    started_at_ns: i64,
    ended_at_ns: i64,
    probe_samples: &[(String, Vec<ActivityResult>)],
) -> Option<PostResult> {
    if probe_samples.is_empty() {
        return None;
    }

    // Nanosecond delta converted to seconds; i64 → f64 precision loss is
    // acceptable for human-readable post-phase duration display.
    #[allow(clippy::cast_precision_loss)]
    let duration_s = (ended_at_ns - started_at_ns) as f64 / 1_000_000_000.0;
    // Total sample counts in chaos experiments are always << u32::MAX.
    #[allow(clippy::cast_possible_truncation)]
    let total_samples = probe_samples.iter().map(|(_, s)| s.len()).sum::<usize>() as u32;

    let probes: Vec<ProbePost> = probe_samples
        .iter()
        .map(|(name, samples)| {
            // u64 → f64 precision loss is acceptable for millisecond statistics display.
            #[allow(clippy::cast_precision_loss)]
            let sample_ms: Vec<f64> = samples.iter().map(|s| s.duration_ms as f64).collect();
            // usize → f64 precision loss is acceptable for mean calculation with small N.
            #[allow(clippy::cast_precision_loss)]
            let mean = if sample_ms.is_empty() {
                0.0
            } else {
                sample_ms.iter().sum::<f64>() / sample_ms.len() as f64
            };
            let mut sorted = sample_ms.clone();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let p95 = if sorted.is_empty() {
                0.0
            } else {
                // Percentile index computation: usize → f64 and f64 → usize casts
                // are acceptable for small sample sizes used in chaos probe sampling.
                #[allow(
                    clippy::cast_precision_loss,
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss
                )]
                let idx = ((sorted.len() as f64 * 0.95) as usize).min(sorted.len() - 1);
                sorted[idx]
            };
            let failed = samples
                .iter()
                .filter(|s| s.status == ActivityStatus::Failed)
                .count();
            // usize → f64 precision loss is acceptable for error rate display.
            #[allow(clippy::cast_precision_loss)]
            let error_rate = if samples.is_empty() {
                0.0
            } else {
                failed as f64 / samples.len() as f64
            };
            let all_succeeded = failed == 0;
            let recovery_time_s = if all_succeeded {
                0.0
            } else {
                let last_failure_ns = samples
                    .iter()
                    .rev()
                    .find(|s| s.status == ActivityStatus::Failed)
                    .map_or(started_at_ns, |s| s.started_at_ns);
                // Nanosecond delta to seconds; i64 → f64 precision loss acceptable
                // for human-readable recovery time display.
                #[allow(clippy::cast_precision_loss)]
                let secs = (last_failure_ns - started_at_ns) as f64 / 1_000_000_000.0;
                secs
            };

            ProbePost {
                name: name.clone(),
                mean,
                p95,
                error_rate,
                returned_to_baseline: all_succeeded,
                recovery_time_s,
            }
        })
        .collect();

    let full_recovery = probes.iter().all(|p| p.returned_to_baseline);
    let recovery_time_s = probes
        .iter()
        .map(|p| p.recovery_time_s)
        .fold(0.0_f64, f64::max);

    // MTTR: when full recovery is observed, set to the maximum recovery time
    // across all probes; when recovery was never achieved, leave as None.
    let mttr_s = if full_recovery {
        Some(recovery_time_s)
    } else {
        None
    };

    Some(PostResult {
        started_at_ns,
        ended_at_ns,
        duration_s,
        samples: total_samples,
        probes,
        recovery_time_s,
        full_recovery,
        residual_degradation: None,
        data_integrity_verified: None,
        data_loss_detected: None,
        mttr_s,
    })
}

/// Build a Journal for an experiment interrupted before it started.
fn make_interrupted_journal(experiment: &Experiment, now_ns: i64) -> Journal {
    Journal {
        experiment_title: experiment.title.clone(),
        experiment_id: uuid::Uuid::new_v4().to_string(),
        status: ExperimentStatus::Interrupted,
        started_at_ns: now_ns,
        ended_at_ns: now_ns,
        duration_ms: 0,
        steady_state_before: None,
        steady_state_after: None,
        method_results: vec![],
        rollback_results: vec![],
        rollback_failures: 0,
        estimate: experiment.estimate.clone(),
        baseline_result: None,
        during_result: None,
        post_result: None,
        load_result: None,
        analysis: None,
        regulatory: experiment.regulatory.clone(),
    }
}

/// Extract target attributes from an activity's provider.
fn target_attributes(activity: &Activity) -> Vec<KeyValue> {
    match &activity.provider {
        Provider::Process { path, .. } => vec![
            KeyValue::new("resilience.target.type", "process"),
            KeyValue::new("resilience.target.name", path.clone()),
        ],
        Provider::Http { url, method, .. } => vec![
            KeyValue::new("resilience.target.type", "http"),
            KeyValue::new("resilience.target.name", url.clone()),
            KeyValue::new("resilience.target.endpoint", format!("{method} {url}")),
        ],
        Provider::Native {
            plugin, function, ..
        } => vec![
            KeyValue::new("resilience.target.type", "native"),
            KeyValue::new("resilience.target.name", plugin.clone()),
            KeyValue::new(
                "resilience.target.component",
                format!("{plugin}::{function}"),
            ),
        ],
    }
}

/// Extract fault attributes from an activity.
fn fault_attributes(activity: &Activity) -> Vec<KeyValue> {
    let fault_type = match activity.activity_type {
        ActivityType::Action => "injection",
        ActivityType::Probe => "observation",
    };
    vec![
        KeyValue::new("resilience.fault.type", fault_type),
        KeyValue::new("resilience.fault.name", activity.name.clone()),
    ]
}

/// Set span error status if the outcome failed.
fn set_span_status_from_outcome(success: bool, error: Option<&str>) {
    if !success {
        let ctx = opentelemetry::Context::current();
        let span = ctx.span();
        let desc = error.unwrap_or("activity failed");
        span.set_status(opentelemetry::trace::Status::error(desc.to_string()));
    }
}

/// Get the current trace ID from the active span context.
fn current_trace_id() -> TraceId {
    let ctx = opentelemetry::Context::current();
    let sc = ctx.span().span_context().clone();
    if sc.is_valid() {
        TraceId(sc.trace_id().to_string())
    } else {
        TraceId::empty()
    }
}

/// Get the current span ID from the active span context.
fn current_span_id() -> SpanId {
    let ctx = opentelemetry::Context::current();
    let sc = ctx.span().span_context().clone();
    if sc.is_valid() {
        SpanId(sc.span_id().to_string())
    } else {
        SpanId::empty()
    }
}

/// Gets current time as epoch nanoseconds.
///
/// # Panics
///
/// Panics if the system clock is outside the representable i64 nanosecond range
/// (before 1677 or after 2262).
#[must_use]
pub fn epoch_nanos_now() -> i64 {
    chrono::Utc::now()
        .timestamp_nanos_opt()
        .expect("timestamp overflow: clock outside i64 nanosecond range")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::ControlRegistry;
    use crate::types::{
        Confidence, DegradationLevel, Estimate, HttpMethod, LoadTool, RegulatoryMapping,
        RegulatoryRequirement, Tolerance,
    };
    use indexmap::IndexMap;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // -- Mock executor

    struct MockExecutor {
        success: bool,
        output: Option<String>,
        call_count: Arc<AtomicUsize>,
    }

    impl MockExecutor {
        fn always_succeed() -> Self {
            Self {
                success: true,
                output: Some("200".into()),
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn always_fail() -> Self {
            Self {
                success: false,
                output: None,
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn with_output(output: &str) -> Self {
            Self {
                success: true,
                output: Some(output.into()),
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl ActivityExecutor for MockExecutor {
        fn execute(&self, _activity: &Activity) -> ActivityOutcome {
            self.call_count.fetch_add(1, Ordering::Relaxed);
            ActivityOutcome {
                success: self.success,
                output: self.output.clone(),
                error: if self.success {
                    None
                } else {
                    Some("execution failed".into())
                },
                duration_ms: 42,
            }
        }
    }

    // -- Mock control handler

    struct EventRecorder {
        events: Arc<std::sync::Mutex<Vec<LifecycleEvent>>>,
    }

    impl EventRecorder {
        fn new() -> (Self, Arc<std::sync::Mutex<Vec<LifecycleEvent>>>) {
            let events = Arc::new(std::sync::Mutex::new(Vec::new()));
            (
                Self {
                    events: events.clone(),
                },
                events,
            )
        }
    }

    impl crate::controls::ControlHandler for EventRecorder {
        // Trait returns &str; literal impls appear static but trait sig cannot change
        // because other impls (e.g. CountingHandler) return non-static field refs.
        #[allow(clippy::unnecessary_literal_bound)]
        fn name(&self) -> &str {
            "event-recorder"
        }
        fn on_event(&self, event: &LifecycleEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    // -- Test helpers

    fn test_action(name: &str) -> Activity {
        Activity {
            name: name.into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: "test".into(),
                function: "noop".into(),
                arguments: HashMap::new(),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }
    }

    fn test_action_background(name: &str) -> Activity {
        Activity {
            name: name.into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: "test".into(),
                function: "noop".into(),
                arguments: HashMap::new(),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: true,
            label_selector: None,
        }
    }

    fn test_probe(name: &str) -> Activity {
        Activity {
            name: name.into(),
            activity_type: ActivityType::Probe,
            provider: Provider::Http {
                method: HttpMethod::Get,
                url: "http://localhost/health".into(),
                headers: HashMap::new(),
                body: None,
                timeout_s: Some(5.0),
            },
            tolerance: Some(Tolerance::Exact {
                value: serde_json::Value::Number(200.into()),
            }),
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }
    }

    fn minimal_experiment() -> Experiment {
        Experiment {
            version: "v1".into(),
            title: "Test experiment".into(),
            description: None,
            tags: vec![],
            configuration: IndexMap::new(),
            secrets: IndexMap::new(),
            controls: vec![],
            steady_state_hypothesis: None,
            method: vec![test_action("action-1")],
            rollbacks: vec![],
            estimate: None,
            baseline: None,
            load: None,
            regulatory: None,
        }
    }

    fn experiment_with_hypothesis() -> Experiment {
        let mut exp = minimal_experiment();
        exp.steady_state_hypothesis = Some(Hypothesis {
            title: "System is healthy".into(),
            probes: vec![test_probe("health-check")],
        });
        exp
    }

    fn default_config() -> RunConfig {
        RunConfig::default()
    }

    // -- Tests: basic execution

    #[test]
    fn run_minimal_experiment_succeeds() {
        let exp = minimal_experiment();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Completed);
        assert_eq!(journal.method_results.len(), 1);
        assert_eq!(journal.method_results[0].name, "action-1");
        assert_eq!(journal.method_results[0].status, ActivityStatus::Succeeded);
    }

    #[test]
    fn empty_method_returns_error() {
        let mut exp = minimal_experiment();
        exp.method = vec![];
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let result = run_experiment(&exp, &executor, &controls, &default_config());
        assert!(result.is_err());
    }

    #[test]
    fn multiple_method_steps_all_execute() {
        let mut exp = minimal_experiment();
        exp.method = vec![
            test_action("step-1"),
            test_action("step-2"),
            test_action("step-3"),
        ];
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.method_results.len(), 3);
        assert_eq!(journal.status, ExperimentStatus::Completed);
    }

    #[test]
    fn failed_action_marks_experiment_failed() {
        let exp = minimal_experiment();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_fail());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Failed);
    }

    // -- Tests: hypothesis evaluation

    #[test]
    fn hypothesis_before_pass_allows_execution() {
        let exp = experiment_with_hypothesis();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::with_output("200"));
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Completed);
        assert!(journal.steady_state_before.is_some());
        assert!(journal.steady_state_before.as_ref().unwrap().met);
    }

    #[test]
    fn hypothesis_before_fail_aborts_experiment() {
        let exp = experiment_with_hypothesis();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::with_output("500"));
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Aborted);
        assert!(journal.steady_state_before.is_some());
        assert!(!journal.steady_state_before.as_ref().unwrap().met);
        assert!(journal.method_results.is_empty());
    }

    #[test]
    fn hypothesis_after_fail_marks_deviated() {
        struct AlternatingExecutor {
            call_count: Arc<AtomicUsize>,
        }
        impl ActivityExecutor for AlternatingExecutor {
            fn execute(&self, _activity: &Activity) -> ActivityOutcome {
                let count = self.call_count.fetch_add(1, Ordering::Relaxed);
                let output = if count == 2 { "500" } else { "200" };
                ActivityOutcome {
                    success: true,
                    output: Some(output.into()),
                    error: None,
                    duration_ms: 10,
                }
            }
        }

        let exp = experiment_with_hypothesis();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(AlternatingExecutor {
            call_count: Arc::new(AtomicUsize::new(0)),
        });
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Deviated);
        assert!(journal.steady_state_after.is_some());
        assert!(!journal.steady_state_after.as_ref().unwrap().met);
    }

    // -- Tests: rollback execution

    #[test]
    fn rollbacks_execute_on_deviation_with_default_strategy() {
        struct DeviatingExecutor {
            call_count: Arc<AtomicUsize>,
        }
        impl ActivityExecutor for DeviatingExecutor {
            fn execute(&self, _activity: &Activity) -> ActivityOutcome {
                let count = self.call_count.fetch_add(1, Ordering::Relaxed);
                let output = if count == 2 { "500" } else { "200" };
                ActivityOutcome {
                    success: true,
                    output: Some(output.into()),
                    error: None,
                    duration_ms: 10,
                }
            }
        }

        let mut exp = experiment_with_hypothesis();
        exp.rollbacks = vec![test_action("rollback-1")];
        let executor: Arc<dyn ActivityExecutor> = Arc::new(DeviatingExecutor {
            call_count: Arc::new(AtomicUsize::new(0)),
        });
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Deviated);
        assert_eq!(journal.rollback_results.len(), 1);
        assert_eq!(journal.rollback_results[0].name, "rollback-1");
    }

    #[test]
    fn rollbacks_skipped_with_never_strategy() {
        let mut exp = minimal_experiment();
        exp.rollbacks = vec![test_action("rollback-1")];
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_fail());
        let controls = Arc::new(ControlRegistry::new());
        let config = RunConfig {
            rollback_strategy: RollbackStrategy::Never,
            cancellation_token: None,
            parent_context: None,
            load_executor: None,
        };

        let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

        assert!(journal.rollback_results.is_empty());
    }

    #[test]
    fn rollbacks_execute_always_strategy() {
        let mut exp = minimal_experiment();
        exp.rollbacks = vec![test_action("rollback-1")];
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());
        let config = RunConfig {
            rollback_strategy: RollbackStrategy::Always,
            cancellation_token: None,
            parent_context: None,
            load_executor: None,
        };

        let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

        assert_eq!(journal.rollback_results.len(), 1);
    }

    // -- Tests: controls lifecycle

    #[test]
    fn controls_emit_before_after_experiment() {
        let exp = minimal_experiment();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let mut controls = ControlRegistry::new();
        let (recorder, events) = EventRecorder::new();
        controls.register(Box::new(recorder));
        let controls = Arc::new(controls);

        run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        let events = events.lock().unwrap();
        assert_eq!(events.first(), Some(&LifecycleEvent::BeforeExperiment));
        assert_eq!(events.last(), Some(&LifecycleEvent::AfterExperiment));
    }

    #[test]
    fn controls_emit_before_after_method() {
        let exp = minimal_experiment();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let mut controls = ControlRegistry::new();
        let (recorder, events) = EventRecorder::new();
        controls.register(Box::new(recorder));
        let controls = Arc::new(controls);

        run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        let events = events.lock().unwrap();
        assert!(events.contains(&LifecycleEvent::BeforeMethod));
        assert!(events.contains(&LifecycleEvent::AfterMethod));
    }

    #[test]
    fn controls_emit_before_after_activity() {
        let exp = minimal_experiment();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let mut controls = ControlRegistry::new();
        let (recorder, events) = EventRecorder::new();
        controls.register(Box::new(recorder));
        let controls = Arc::new(controls);

        run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        let events = events.lock().unwrap();
        assert!(events.contains(&LifecycleEvent::BeforeActivity {
            name: "action-1".into()
        }));
        assert!(events.contains(&LifecycleEvent::AfterActivity {
            name: "action-1".into()
        }));
    }

    #[test]
    fn controls_emit_hypothesis_events() {
        let exp = experiment_with_hypothesis();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::with_output("200"));
        let mut controls = ControlRegistry::new();
        let (recorder, events) = EventRecorder::new();
        controls.register(Box::new(recorder));
        let controls = Arc::new(controls);

        run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        let events = events.lock().unwrap();
        let hypothesis_events: Vec<_> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    LifecycleEvent::BeforeHypothesis | LifecycleEvent::AfterHypothesis
                )
            })
            .collect();
        assert_eq!(hypothesis_events.len(), 4);
    }

    #[test]
    fn controls_emit_rollback_events_when_rollbacks_execute() {
        let mut exp = minimal_experiment();
        exp.rollbacks = vec![test_action("rollback-1")];
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let mut controls = ControlRegistry::new();
        let (recorder, events) = EventRecorder::new();
        controls.register(Box::new(recorder));
        let controls = Arc::new(controls);
        let config = RunConfig {
            rollback_strategy: RollbackStrategy::Always,
            cancellation_token: None,
            parent_context: None,
            load_executor: None,
        };

        run_experiment(&exp, &executor, &controls, &config).unwrap();

        let events = events.lock().unwrap();
        assert!(events.contains(&LifecycleEvent::BeforeRollback));
        assert!(events.contains(&LifecycleEvent::AfterRollback));
    }

    #[test]
    fn full_lifecycle_event_order() {
        let mut exp = experiment_with_hypothesis();
        exp.rollbacks = vec![test_action("rollback-1")];
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::with_output("200"));
        let mut controls = ControlRegistry::new();
        let (recorder, events) = EventRecorder::new();
        controls.register(Box::new(recorder));
        let controls = Arc::new(controls);
        let config = RunConfig {
            rollback_strategy: RollbackStrategy::Always,
            cancellation_token: None,
            parent_context: None,
            load_executor: None,
        };

        run_experiment(&exp, &executor, &controls, &config).unwrap();

        let events = events.lock().unwrap();
        let event_names: Vec<&str> = events
            .iter()
            .map(|e| match e {
                LifecycleEvent::BeforeExperiment => "BeforeExperiment",
                LifecycleEvent::AfterExperiment => "AfterExperiment",
                LifecycleEvent::BeforeMethod => "BeforeMethod",
                LifecycleEvent::AfterMethod => "AfterMethod",
                LifecycleEvent::BeforeHypothesis => "BeforeHypothesis",
                LifecycleEvent::AfterHypothesis => "AfterHypothesis",
                LifecycleEvent::BeforeActivity { .. } => "BeforeActivity",
                LifecycleEvent::AfterActivity { .. } => "AfterActivity",
                LifecycleEvent::BeforeRollback => "BeforeRollback",
                LifecycleEvent::AfterRollback => "AfterRollback",
            })
            .collect();

        let exp_idx = event_names
            .iter()
            .position(|&e| e == "BeforeExperiment")
            .unwrap();
        let hyp_before_idx = event_names
            .iter()
            .position(|&e| e == "BeforeHypothesis")
            .unwrap();
        let method_idx = event_names
            .iter()
            .position(|&e| e == "BeforeMethod")
            .unwrap();
        let rollback_idx = event_names
            .iter()
            .position(|&e| e == "BeforeRollback")
            .unwrap();
        let exp_end_idx = event_names
            .iter()
            .position(|&e| e == "AfterExperiment")
            .unwrap();

        assert!(exp_idx < hyp_before_idx);
        assert!(hyp_before_idx < method_idx);
        assert!(method_idx < rollback_idx);
        assert!(rollback_idx < exp_end_idx);
    }

    // -- Tests: estimate and analysis

    #[test]
    fn estimate_preserved_in_journal() {
        let mut exp = minimal_experiment();
        exp.estimate = Some(Estimate {
            expected_outcome: ExpectedOutcome::Recovered,
            expected_recovery_s: Some(15.0),
            expected_degradation: Some(DegradationLevel::Moderate),
            expected_data_loss: Some(false),
            confidence: Some(Confidence::High),
            rationale: Some("tested before".into()),
            prior_runs: Some(5),
        });
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert!(journal.estimate.is_some());
        assert_eq!(
            journal.estimate.as_ref().unwrap().expected_outcome,
            ExpectedOutcome::Recovered
        );
    }

    #[test]
    fn analysis_computed_when_estimate_present() {
        let mut exp = minimal_experiment();
        exp.estimate = Some(Estimate {
            expected_outcome: ExpectedOutcome::Recovered,
            expected_recovery_s: Some(15.0),
            expected_degradation: None,
            expected_data_loss: None,
            confidence: Some(Confidence::High),
            rationale: None,
            prior_runs: None,
        });
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert!(journal.analysis.is_some());
        assert_eq!(
            journal.analysis.as_ref().unwrap().estimate_accuracy,
            Some(1.0)
        );
        assert_eq!(
            journal.analysis.as_ref().unwrap().resilience_score,
            Some(1.0)
        );
    }

    #[test]
    fn analysis_not_present_without_estimate() {
        let exp = minimal_experiment();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert!(journal.analysis.is_none());
    }

    // -- Tests: journal metadata

    #[test]
    fn journal_has_correct_title() {
        let exp = minimal_experiment();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.experiment_title, "Test experiment");
    }

    #[test]
    fn journal_has_valid_timestamps() {
        let exp = minimal_experiment();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert!(journal.started_at_ns > 0);
        assert!(journal.ended_at_ns >= journal.started_at_ns);
    }

    #[test]
    fn journal_has_uuid_experiment_id() {
        let exp = minimal_experiment();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.experiment_id.len(), 36);
        assert!(journal.experiment_id.contains('-'));
    }

    #[test]
    fn regulatory_preserved_in_journal() {
        let mut exp = minimal_experiment();
        exp.regulatory = Some(RegulatoryMapping {
            frameworks: vec!["DORA".into()],
            requirements: vec![RegulatoryRequirement {
                id: "DORA-Art24".into(),
                description: "ICT resilience testing".into(),
                evidence: "Recovery within RTO".into(),
            }],
        });
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert!(journal.regulatory.is_some());
    }

    // -- Tests: abort with rollback

    struct AbortThenSucceedExecutor {
        call_count: Arc<AtomicUsize>,
    }
    impl ActivityExecutor for AbortThenSucceedExecutor {
        fn execute(&self, _activity: &Activity) -> ActivityOutcome {
            let count = self.call_count.fetch_add(1, Ordering::Relaxed);
            if count == 0 {
                ActivityOutcome {
                    success: true,
                    output: Some("500".into()),
                    error: None,
                    duration_ms: 10,
                }
            } else {
                ActivityOutcome {
                    success: true,
                    output: Some("200".into()),
                    error: None,
                    duration_ms: 10,
                }
            }
        }
    }

    #[test]
    fn aborted_experiment_runs_rollbacks_on_deviation_strategy() {
        let mut exp = experiment_with_hypothesis();
        exp.rollbacks = vec![test_action("cleanup")];

        let executor: Arc<dyn ActivityExecutor> = Arc::new(AbortThenSucceedExecutor {
            call_count: Arc::new(AtomicUsize::new(0)),
        });
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Aborted);
        assert_eq!(journal.rollback_results.len(), 1);
    }

    // -- Tests: cancellation token

    #[test]
    fn cancelled_token_returns_interrupted_status() {
        let exp = minimal_experiment();
        let mock = MockExecutor::always_succeed();
        let call_count = mock.call_count.clone();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(mock);
        let controls = Arc::new(ControlRegistry::new());

        let token = CancellationToken::new();
        token.cancel();

        let config = RunConfig {
            rollback_strategy: RollbackStrategy::OnDeviation,
            cancellation_token: Some(token),
            parent_context: None,
            load_executor: None,
        };

        let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Interrupted);
        assert!(journal.method_results.is_empty());
        assert_eq!(call_count.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn none_cancellation_token_runs_normally() {
        let exp = minimal_experiment();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let config = RunConfig {
            rollback_strategy: RollbackStrategy::OnDeviation,
            cancellation_token: None,
            parent_context: None,
            load_executor: None,
        };

        let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Completed);
    }

    // -- Tests: failed rollback handling

    #[test]
    fn failed_rollback_continues_and_counts_failures() {
        struct MethodSucceedRollbackFailExecutor {
            call_count: Arc<AtomicUsize>,
        }
        impl ActivityExecutor for MethodSucceedRollbackFailExecutor {
            fn execute(&self, activity: &Activity) -> ActivityOutcome {
                self.call_count.fetch_add(1, Ordering::Relaxed);
                if activity.name.starts_with("rollback") {
                    ActivityOutcome {
                        success: false,
                        output: None,
                        error: Some("rollback failed".into()),
                        duration_ms: 10,
                    }
                } else {
                    ActivityOutcome {
                        success: true,
                        output: Some("200".into()),
                        error: None,
                        duration_ms: 10,
                    }
                }
            }
        }

        let mut exp = minimal_experiment();
        exp.rollbacks = vec![test_action("rollback-1"), test_action("rollback-2")];
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MethodSucceedRollbackFailExecutor {
            call_count: Arc::new(AtomicUsize::new(0)),
        });
        let controls = Arc::new(ControlRegistry::new());
        let config = RunConfig {
            rollback_strategy: RollbackStrategy::Always,
            cancellation_token: None,
            parent_context: None,
            load_executor: None,
        };

        let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Completed);
        assert_eq!(journal.rollback_results.len(), 2);
        assert_eq!(journal.rollback_failures, 2);
    }

    // -- Tests: background task spawning

    #[test]
    fn background_activities_are_executed() {
        let mut exp = minimal_experiment();
        exp.method = vec![
            test_action("fg-1"),
            test_action_background("bg-1"),
            test_action_background("bg-2"),
        ];
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Completed);
        assert_eq!(journal.method_results.len(), 3);

        let names: Vec<&str> = journal
            .method_results
            .iter()
            .map(|r| r.name.as_str())
            .collect();
        assert!(names.contains(&"fg-1"));
        assert!(names.contains(&"bg-1"));
        assert!(names.contains(&"bg-2"));
    }

    #[test]
    fn background_and_foreground_both_counted_in_results() {
        let mut exp = minimal_experiment();
        exp.method = vec![
            test_action("fg-1"),
            test_action("fg-2"),
            test_action_background("bg-1"),
        ];
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.method_results.len(), 3);
        // Foreground results appear first, background after
        assert_eq!(journal.method_results[0].name, "fg-1");
        assert_eq!(journal.method_results[1].name, "fg-2");
        assert_eq!(journal.method_results[2].name, "bg-1");
    }

    #[test]
    fn all_background_activities_still_execute() {
        let mut exp = minimal_experiment();
        exp.method = vec![
            test_action_background("bg-1"),
            test_action_background("bg-2"),
            test_action_background("bg-3"),
        ];
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.method_results.len(), 3);
        assert_eq!(journal.status, ExperimentStatus::Completed);
    }

    #[test]
    fn background_activity_failure_reflected_in_results() {
        struct NameBasedExecutor;
        impl ActivityExecutor for NameBasedExecutor {
            fn execute(&self, activity: &Activity) -> ActivityOutcome {
                if activity.name == "bg-fail" {
                    ActivityOutcome {
                        success: false,
                        output: None,
                        error: Some("bg failed".into()),
                        duration_ms: 5,
                    }
                } else {
                    ActivityOutcome {
                        success: true,
                        output: Some("ok".into()),
                        error: None,
                        duration_ms: 5,
                    }
                }
            }
        }

        let mut exp = minimal_experiment();
        exp.method = vec![test_action("fg-ok"), test_action_background("bg-fail")];
        let executor: Arc<dyn ActivityExecutor> = Arc::new(NameBasedExecutor);
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.method_results.len(), 2);
        let bg_result = journal
            .method_results
            .iter()
            .find(|r| r.name == "bg-fail")
            .unwrap();
        assert_eq!(bg_result.status, ActivityStatus::Failed);
    }

    #[test]
    fn background_executor_call_count_matches() {
        let mut exp = minimal_experiment();
        exp.method = vec![
            test_action("fg-1"),
            test_action_background("bg-1"),
            test_action_background("bg-2"),
        ];
        let mock = MockExecutor::always_succeed();
        let call_count = mock.call_count.clone();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(mock);
        let controls = Arc::new(ControlRegistry::new());

        run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        // All 3 activities should have been executed
        assert_eq!(call_count.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn pause_before_and_after_emits_span_events_without_panic() {
        // Verify that pause_before_s / pause_after_s paths do not panic and
        // that the OTel span event calls complete without error.
        // We use a very small duration (near-zero) so the test is fast.
        let mut exp = minimal_experiment();
        let mut activity = test_action("paused-step");
        // Non-positive pause is skipped, so use a tiny positive value.
        activity.pause_before_s = Some(0.001);
        activity.pause_after_s = Some(0.001);
        exp.method = vec![activity];

        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();
        assert_eq!(journal.method_results.len(), 1);
        assert_eq!(journal.method_results[0].status, ActivityStatus::Succeeded);
    }

    // -- Tests: during-phase sampling and MTTR (F4)

    #[test]
    fn during_phase_samples_are_collected() {
        // Arrange: experiment with hypothesis — runner should populate during_result
        let exp = experiment_with_hypothesis();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::with_output("200"));
        let controls = Arc::new(ControlRegistry::new());

        // Act
        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        // Assert: during_result is present with at least one probe
        assert!(
            journal.during_result.is_some(),
            "during_result should be populated when hypothesis is present"
        );
        let during = journal.during_result.as_ref().unwrap();
        assert!(
            !during.probes.is_empty(),
            "during_result should have at least one probe entry"
        );
        assert!(
            during.probes[0].samples > 0,
            "during probe should have at least one sample"
        );
    }

    #[test]
    fn mttr_calculated_on_recovery() {
        // Arrange: executor that always succeeds — all post-phase samples succeed
        // so system is immediately "recovered", and mttr_s should be Some(...)
        let exp = experiment_with_hypothesis();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::with_output("200"));
        let controls = Arc::new(ControlRegistry::new());

        // Act
        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        // Assert: post_result.mttr_s is populated
        assert!(
            journal.post_result.is_some(),
            "post_result should be populated when hypothesis is present"
        );
        let post = journal.post_result.as_ref().unwrap();
        assert!(
            post.mttr_s.is_some(),
            "mttr_s should be Some when post-phase probes are collected"
        );
        assert!(post.mttr_s.unwrap() >= 0.0, "mttr_s must be non-negative");
    }

    #[test]
    fn run_experiment_emits_audit_log_without_panic() {
        // Verifies the audit tracing::info! calls don't panic and the
        // experiment completes normally (structured fields are correct types).
        let exp = minimal_experiment();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());
        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();
        assert_eq!(journal.status, ExperimentStatus::Completed);
        assert!(!journal.experiment_id.is_empty());
    }

    // -- Load testing phase tests

    struct MockLoadExecutor {
        started: Arc<std::sync::Mutex<bool>>,
        stopped: Arc<std::sync::Mutex<bool>>,
    }

    impl MockLoadExecutor {
        fn new() -> (
            Self,
            Arc<std::sync::Mutex<bool>>,
            Arc<std::sync::Mutex<bool>>,
        ) {
            let started = Arc::new(std::sync::Mutex::new(false));
            let stopped = Arc::new(std::sync::Mutex::new(false));
            (
                Self {
                    started: started.clone(),
                    stopped: stopped.clone(),
                },
                started,
                stopped,
            )
        }
    }

    impl LoadExecutor for MockLoadExecutor {
        fn start(&self, _config: &LoadConfig) -> Result<LoadHandle, String> {
            *self.started.lock().expect("lock") = true;
            Ok(LoadHandle {
                inner: Box::new(()),
            })
        }

        fn stop(&self, _handle: LoadHandle) -> Result<LoadResult, String> {
            *self.stopped.lock().expect("lock") = true;
            Ok(LoadResult {
                tool: LoadTool::K6,
                started_at_ns: 1_000_000_000,
                ended_at_ns: 2_000_000_000,
                duration_s: 1.0,
                vus: 5,
                throughput_rps: 100.0,
                latency_p50_ms: 10.0,
                latency_p95_ms: 50.0,
                latency_p99_ms: 100.0,
                error_rate: 0.01,
                total_requests: 100,
                thresholds_met: true,
            })
        }
    }

    fn experiment_with_load() -> Experiment {
        let mut exp = experiment_with_hypothesis();
        exp.load = Some(LoadConfig {
            tool: LoadTool::K6,
            script: std::path::PathBuf::from("test.js"),
            vus: Some(5),
            duration_s: Some(10.0),
            thresholds: HashMap::new(),
        });
        exp
    }

    #[test]
    fn load_result_none_when_no_load_config() {
        let exp = minimal_experiment();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());
        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert!(journal.load_result.is_none());
    }

    #[test]
    fn load_result_populated_when_load_executor_present() {
        let exp = experiment_with_load();
        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());
        let (mock_load, started, stopped) = MockLoadExecutor::new();

        let config = RunConfig {
            load_executor: Some(Arc::new(mock_load)),
            ..RunConfig::default()
        };

        let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

        // Load executor was called
        assert!(
            *started.lock().expect("lock"),
            "load should have been started"
        );
        assert!(
            *stopped.lock().expect("lock"),
            "load should have been stopped"
        );

        // Load result populated in journal
        assert!(
            journal.load_result.is_some(),
            "journal should have load_result"
        );
        let lr = journal.load_result.as_ref().expect("load_result");
        assert_eq!(lr.vus, 5);
        assert_eq!(lr.total_requests, 100);
        assert!(lr.thresholds_met);
    }

    #[test]
    fn load_not_started_when_hypothesis_fails() {
        let mut exp = experiment_with_load();
        // Make hypothesis tolerance impossible
        if let Some(ref mut hyp) = exp.steady_state_hypothesis {
            hyp.probes[0].tolerance = Some(Tolerance::Regex {
                pattern: "^IMPOSSIBLE$".into(),
            });
        }

        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());
        let (mock_load, started, _stopped) = MockLoadExecutor::new();

        let config = RunConfig {
            load_executor: Some(Arc::new(mock_load)),
            ..RunConfig::default()
        };

        let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Aborted);
        assert!(
            !*started.lock().expect("lock"),
            "load should NOT start when hypothesis fails"
        );
        assert!(journal.load_result.is_none());
    }

    // -- GameDay runner tests

    #[test]
    fn gameday_runs_all_experiments() {
        use crate::types::{GameDay, GameDayExperiment, ScoringConfig};

        let gameday = GameDay {
            title: "Test GameDay".into(),
            description: None,
            tags: vec![],
            regulatory: None,
            load: None,
            experiments: vec![
                GameDayExperiment {
                    path: "exp1.toon".into(),
                    compliance_maps: vec![],
                },
                GameDayExperiment {
                    path: "exp2.toon".into(),
                    compliance_maps: vec![],
                },
            ],
            scoring: ScoringConfig::default(),
        };

        let exp1 = minimal_experiment();
        let mut exp2 = minimal_experiment();
        exp2.title = "Second experiment".into();

        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let result = run_gameday(
            &gameday,
            &[exp1, exp2],
            &executor,
            &controls,
            &default_config(),
        )
        .expect("gameday should succeed");

        assert_eq!(result.experiment_journals.len(), 2);
        assert_eq!(result.title, "Test GameDay");
        assert!(result.resilience_score.overall > 0.0);
        assert_eq!(result.compliance_status, "COMPLIANT");
    }

    #[test]
    fn gameday_score_reflects_failures() {
        use crate::types::{GameDay, GameDayExperiment, ScoringConfig};

        let gameday = GameDay {
            title: "Mixed GameDay".into(),
            description: None,
            tags: vec![],
            regulatory: None,
            load: None,
            experiments: vec![
                GameDayExperiment {
                    path: "pass.toon".into(),
                    compliance_maps: vec!["ART-1".into()],
                },
                GameDayExperiment {
                    path: "fail.toon".into(),
                    compliance_maps: vec!["ART-2".into()],
                },
            ],
            scoring: ScoringConfig::default(),
        };

        let exp_pass = minimal_experiment();
        // Failing experiment: empty method triggers RunnerError, so use
        // a hypothesis that will fail instead
        let mut exp_fail = experiment_with_hypothesis();
        if let Some(ref mut hyp) = exp_fail.steady_state_hypothesis {
            hyp.probes[0].tolerance = Some(Tolerance::Regex {
                pattern: "^NEVER_MATCH$".into(),
            });
        }

        let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
        let controls = Arc::new(ControlRegistry::new());

        let result = run_gameday(
            &gameday,
            &[exp_pass, exp_fail],
            &executor,
            &controls,
            &default_config(),
        )
        .expect("gameday should succeed even with deviations");

        assert_eq!(result.experiment_journals.len(), 2);
        // One completed, one aborted → pass_rate = 0.5
        assert!(result.resilience_score.pass_rate < 1.0);
        // Compliance: ART-1 met (pass), ART-2 not met (fail) → 0.5
        assert!(result.resilience_score.compliance_coverage < 1.0);
        // Overall = 0.5*0.3 + 1.0*0.25 + 1.0*0.25 + 0.5*0.2 = 0.75 → PARTIAL
        assert_eq!(result.compliance_status, "PARTIAL");
    }
}
