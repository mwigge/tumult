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
    all_succeeded, make_result, should_rollback, ResultParams, RollbackStrategy,
};
use crate::types::*;

use opentelemetry::trace::TraceContextExt;
use thiserror::Error;

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

/// Trait for executing activities — allows mocking in tests.
pub trait ActivityExecutor: Send + Sync {
    fn execute(&self, activity: &Activity) -> ActivityOutcome;
}

/// Configuration for an experiment run.
///
/// Dry-run and baseline-skip are handled at the CLI layer before
/// calling `run_experiment`, so they are not part of this config.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub rollback_strategy: RollbackStrategy,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self {
            rollback_strategy: RollbackStrategy::OnDeviation,
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
/// # Examples
///
/// ```
/// use tumult_core::runner::{
///     run_experiment, ActivityExecutor, ActivityOutcome, RunConfig,
/// };
/// use tumult_core::controls::ControlRegistry;
/// use tumult_core::types::*;
/// use std::collections::HashMap;
///
/// // A mock executor that always succeeds
/// struct MockExecutor;
/// impl ActivityExecutor for MockExecutor {
///     fn execute(&self, _activity: &Activity) -> ActivityOutcome {
///         ActivityOutcome {
///             success: true,
///             output: Some("ok".into()),
///             error: None,
///             duration_ms: 10,
///         }
///     }
/// }
///
/// let experiment = Experiment {
///     title: "demo".into(),
///     description: None,
///     tags: vec![],
///     configuration: HashMap::new(),
///     secrets: HashMap::new(),
///     controls: vec![],
///     steady_state_hypothesis: None,
///     method: vec![Activity {
///         name: "noop-action".into(),
///         activity_type: ActivityType::Action,
///         provider: Provider::Native {
///             plugin: "test".into(),
///             function: "noop".into(),
///             arguments: HashMap::new(),
///         },
///         tolerance: None,
///         pause_before_s: None,
///         pause_after_s: None,
///         background: false,
///     }],
///     rollbacks: vec![],
///     estimate: None,
///     baseline: None,
///     load: None,
///     regulatory: None,
/// };
///
/// let journal = run_experiment(
///     &experiment,
///     &MockExecutor,
///     &ControlRegistry::new(),
///     &RunConfig::default(),
/// )
/// .unwrap();
///
/// assert_eq!(journal.status, ExperimentStatus::Completed);
/// ```
pub fn run_experiment(
    experiment: &Experiment,
    executor: &dyn ActivityExecutor,
    controls: &ControlRegistry,
    config: &RunConfig,
) -> Result<Journal, RunnerError> {
    if experiment.method.is_empty() {
        return Err(RunnerError::EmptyMethod);
    }

    let started = Instant::now();
    let started_at_ns = epoch_nanos_now();
    let experiment_id = uuid::Uuid::new_v4().to_string();

    // ── Phase 0: Record Estimate ──────────────────────────────
    controls.emit(&LifecycleEvent::BeforeExperiment);

    // ── Phase 1: Baseline (skipped if configured or no baseline config) ──
    // Baseline acquisition is handled externally; we record the estimate.

    // ── Hypothesis BEFORE ─────────────────────────────────────
    let hypothesis_before = if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        controls.emit(&LifecycleEvent::BeforeHypothesis);
        let result = evaluate_hypothesis(hypothesis, executor, controls);
        controls.emit(&LifecycleEvent::AfterHypothesis);
        Some(result)
    } else {
        None
    };

    let hypothesis_before_met = hypothesis_before.as_ref().map(|h| h.met);

    // If hypothesis before failed, abort — skip method, go to rollbacks
    if hypothesis_before_met == Some(false) {
        let ended_at_ns = epoch_nanos_now();
        let duration_ms = started.elapsed().as_millis() as u64;

        // Run rollbacks if strategy says so and there are rollbacks to run
        let rollback_results = if !experiment.rollbacks.is_empty()
            && should_rollback(&config.rollback_strategy, true)
        {
            controls.emit(&LifecycleEvent::BeforeRollback);
            let results = execute_activities(&experiment.rollbacks, executor, controls);
            controls.emit(&LifecycleEvent::AfterRollback);
            results
        } else {
            vec![]
        };

        controls.emit(&LifecycleEvent::AfterExperiment);

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
            estimate: experiment.estimate.clone(),
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: experiment.regulatory.clone(),
        });
    }

    // ── Phase 2: Execute Method (DURING) ──────────────────────
    controls.emit(&LifecycleEvent::BeforeMethod);
    let method_results = execute_activities(&experiment.method, executor, controls);
    controls.emit(&LifecycleEvent::AfterMethod);

    let actions_succeeded = all_succeeded(&method_results);

    // ── Phase 3: POST — recovery measurement ──────────────────
    // Post-phase sampling is done externally; hypothesis after captures it.

    // ── Hypothesis AFTER ──────────────────────────────────────
    let hypothesis_after = if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        controls.emit(&LifecycleEvent::BeforeHypothesis);
        let result = evaluate_hypothesis(hypothesis, executor, controls);
        controls.emit(&LifecycleEvent::AfterHypothesis);
        Some(result)
    } else {
        None
    };

    let hypothesis_after_met = hypothesis_after.as_ref().map(|h| h.met);

    // ── Determine status ──────────────────────────────────────
    let status = determine_status(
        hypothesis_before_met,
        hypothesis_after_met,
        actions_succeeded,
    );

    // ── Rollbacks ─────────────────────────────────────────────
    let deviated = status == ExperimentStatus::Deviated;
    let rollback_results = if !experiment.rollbacks.is_empty()
        && should_rollback(&config.rollback_strategy, deviated)
    {
        controls.emit(&LifecycleEvent::BeforeRollback);
        let results = execute_activities(&experiment.rollbacks, executor, controls);
        controls.emit(&LifecycleEvent::AfterRollback);
        results
    } else {
        vec![]
    };

    // ── Phase 4: Analysis ─────────────────────────────────────
    let analysis = compute_analysis(experiment, &status);

    let ended_at_ns = epoch_nanos_now();
    let duration_ms = started.elapsed().as_millis() as u64;

    controls.emit(&LifecycleEvent::AfterExperiment);

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
        estimate: experiment.estimate.clone(),
        baseline_result: None,
        during_result: None,
        post_result: None,
        load_result: None,
        analysis,
        regulatory: experiment.regulatory.clone(),
    })
}

/// Evaluate a steady-state hypothesis by running its probes.
fn evaluate_hypothesis(
    hypothesis: &Hypothesis,
    executor: &dyn ActivityExecutor,
    controls: &ControlRegistry,
) -> HypothesisResult {
    let mut probe_results = Vec::with_capacity(hypothesis.probes.len());
    let mut all_met = true;

    for probe in &hypothesis.probes {
        controls.emit(&LifecycleEvent::BeforeActivity {
            name: probe.name.clone(),
        });

        let started_at_ns = epoch_nanos_now();
        let outcome = executor.execute(probe);

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
                // Tolerance defined but no output — cannot evaluate, treat as failure
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

/// Execute a list of activities sequentially, emitting control events.
fn execute_activities(
    activities: &[Activity],
    executor: &dyn ActivityExecutor,
    controls: &ControlRegistry,
) -> Vec<ActivityResult> {
    let mut results = Vec::with_capacity(activities.len());

    for activity in activities {
        // Honour pause_before_s
        if let Some(pause) = activity.pause_before_s {
            if pause > 0.0 {
                std::thread::sleep(std::time::Duration::from_secs_f64(pause));
            }
        }

        controls.emit(&LifecycleEvent::BeforeActivity {
            name: activity.name.clone(),
        });

        let started_at_ns = epoch_nanos_now();
        let outcome = executor.execute(activity);

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

        // Honour pause_after_s
        if let Some(pause) = activity.pause_after_s {
            if pause > 0.0 {
                std::thread::sleep(std::time::Duration::from_secs_f64(pause));
            }
        }

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

/// Get the current trace ID from the active span context.
fn current_trace_id() -> String {
    let ctx = opentelemetry::Context::current();
    let sc = ctx.span().span_context().clone();
    if sc.is_valid() {
        sc.trace_id().to_string()
    } else {
        String::new()
    }
}

/// Get the current span ID from the active span context.
fn current_span_id() -> String {
    let ctx = opentelemetry::Context::current();
    let sc = ctx.span().span_context().clone();
    if sc.is_valid() {
        sc.span_id().to_string()
    } else {
        String::new()
    }
}

/// Get current time as epoch nanoseconds.
fn epoch_nanos_now() -> i64 {
    chrono::Utc::now()
        .timestamp_nanos_opt()
        .expect("timestamp overflow: clock outside i64 nanosecond range")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controls::ControlRegistry;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    // ── Mock executor ─────────────────────────────────────────

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

    // ── Mock control handler ──────────────────────────────────

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
        fn name(&self) -> &str {
            "event-recorder"
        }
        fn on_event(&self, event: &LifecycleEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    // ── Test helpers ──────────────────────────────────────────

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
        }
    }

    fn minimal_experiment() -> Experiment {
        Experiment {
            title: "Test experiment".into(),
            description: None,
            tags: vec![],
            configuration: HashMap::new(),
            secrets: HashMap::new(),
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

    // ── Tests: basic execution ────────────────────────────────

    #[test]
    fn run_minimal_experiment_succeeds() {
        let exp = minimal_experiment();
        let executor = MockExecutor::always_succeed();
        let controls = ControlRegistry::new();

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
        let executor = MockExecutor::always_succeed();
        let controls = ControlRegistry::new();

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
        let executor = MockExecutor::always_succeed();
        let controls = ControlRegistry::new();

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.method_results.len(), 3);
        assert_eq!(journal.status, ExperimentStatus::Completed);
    }

    #[test]
    fn failed_action_marks_experiment_failed() {
        let exp = minimal_experiment();
        let executor = MockExecutor::always_fail();
        let controls = ControlRegistry::new();

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Failed);
    }

    // ── Tests: hypothesis evaluation ──────────────────────────

    #[test]
    fn hypothesis_before_pass_allows_execution() {
        let exp = experiment_with_hypothesis();
        let executor = MockExecutor::with_output("200");
        let controls = ControlRegistry::new();

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Completed);
        assert!(journal.steady_state_before.is_some());
        assert!(journal.steady_state_before.as_ref().unwrap().met);
    }

    #[test]
    fn hypothesis_before_fail_aborts_experiment() {
        let exp = experiment_with_hypothesis();
        // Executor returns "500" which doesn't match tolerance (exact: 200)
        let executor = MockExecutor::with_output("500");
        let controls = ControlRegistry::new();

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Aborted);
        assert!(journal.steady_state_before.is_some());
        assert!(!journal.steady_state_before.as_ref().unwrap().met);
        assert!(journal.method_results.is_empty()); // Method should not execute
    }

    #[test]
    fn hypothesis_after_fail_marks_deviated() {
        // Need a custom executor that succeeds for method but fails for second hypothesis
        struct AlternatingExecutor {
            call_count: Arc<AtomicUsize>,
        }
        impl ActivityExecutor for AlternatingExecutor {
            fn execute(&self, _activity: &Activity) -> ActivityOutcome {
                let count = self.call_count.fetch_add(1, Ordering::Relaxed);
                // First hypothesis probe (call 0) passes, method (call 1) passes,
                // second hypothesis probe (call 2) fails
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
        let executor = AlternatingExecutor {
            call_count: Arc::new(AtomicUsize::new(0)),
        };
        let controls = ControlRegistry::new();

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Deviated);
        assert!(journal.steady_state_after.is_some());
        assert!(!journal.steady_state_after.as_ref().unwrap().met);
    }

    // ── Tests: rollback execution ─────────────────────────────

    #[test]
    fn rollbacks_execute_on_deviation_with_default_strategy() {
        struct DeviatingExecutor {
            call_count: Arc<AtomicUsize>,
        }
        impl ActivityExecutor for DeviatingExecutor {
            fn execute(&self, _activity: &Activity) -> ActivityOutcome {
                let count = self.call_count.fetch_add(1, Ordering::Relaxed);
                // hypothesis before (0): pass, method (1): pass,
                // hypothesis after (2): fail, rollback (3): pass
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
        let executor = DeviatingExecutor {
            call_count: Arc::new(AtomicUsize::new(0)),
        };
        let controls = ControlRegistry::new();

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Deviated);
        assert_eq!(journal.rollback_results.len(), 1);
        assert_eq!(journal.rollback_results[0].name, "rollback-1");
    }

    #[test]
    fn rollbacks_skipped_with_never_strategy() {
        let mut exp = minimal_experiment();
        exp.rollbacks = vec![test_action("rollback-1")];
        let executor = MockExecutor::always_fail();
        let controls = ControlRegistry::new();
        let config = RunConfig {
            rollback_strategy: RollbackStrategy::Never,
        };

        let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

        assert!(journal.rollback_results.is_empty());
    }

    #[test]
    fn rollbacks_execute_always_strategy() {
        let mut exp = minimal_experiment();
        exp.rollbacks = vec![test_action("rollback-1")];
        let executor = MockExecutor::always_succeed();
        let controls = ControlRegistry::new();
        let config = RunConfig {
            rollback_strategy: RollbackStrategy::Always,
        };

        let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

        assert_eq!(journal.rollback_results.len(), 1);
    }

    // ── Tests: controls lifecycle ─────────────────────────────

    #[test]
    fn controls_emit_before_after_experiment() {
        let exp = minimal_experiment();
        let executor = MockExecutor::always_succeed();
        let mut controls = ControlRegistry::new();
        let (recorder, events) = EventRecorder::new();
        controls.register(Box::new(recorder));

        run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        let events = events.lock().unwrap();
        assert_eq!(events.first(), Some(&LifecycleEvent::BeforeExperiment));
        assert_eq!(events.last(), Some(&LifecycleEvent::AfterExperiment));
    }

    #[test]
    fn controls_emit_before_after_method() {
        let exp = minimal_experiment();
        let executor = MockExecutor::always_succeed();
        let mut controls = ControlRegistry::new();
        let (recorder, events) = EventRecorder::new();
        controls.register(Box::new(recorder));

        run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        let events = events.lock().unwrap();
        assert!(events.contains(&LifecycleEvent::BeforeMethod));
        assert!(events.contains(&LifecycleEvent::AfterMethod));
    }

    #[test]
    fn controls_emit_before_after_activity() {
        let exp = minimal_experiment();
        let executor = MockExecutor::always_succeed();
        let mut controls = ControlRegistry::new();
        let (recorder, events) = EventRecorder::new();
        controls.register(Box::new(recorder));

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
        let executor = MockExecutor::with_output("200");
        let mut controls = ControlRegistry::new();
        let (recorder, events) = EventRecorder::new();
        controls.register(Box::new(recorder));

        run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        let events = events.lock().unwrap();
        // Should have hypothesis events (before+after for both hypothesis checks)
        let hypothesis_events: Vec<_> = events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    LifecycleEvent::BeforeHypothesis | LifecycleEvent::AfterHypothesis
                )
            })
            .collect();
        assert_eq!(hypothesis_events.len(), 4); // 2 pairs (before + after hypothesis)
    }

    #[test]
    fn controls_emit_rollback_events_when_rollbacks_execute() {
        let mut exp = minimal_experiment();
        exp.rollbacks = vec![test_action("rollback-1")];
        let executor = MockExecutor::always_succeed();
        let mut controls = ControlRegistry::new();
        let (recorder, events) = EventRecorder::new();
        controls.register(Box::new(recorder));
        let config = RunConfig {
            rollback_strategy: RollbackStrategy::Always,
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
        let executor = MockExecutor::with_output("200");
        let mut controls = ControlRegistry::new();
        let (recorder, events) = EventRecorder::new();
        controls.register(Box::new(recorder));
        let config = RunConfig {
            rollback_strategy: RollbackStrategy::Always,
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

        // Verify ordering: experiment → hypothesis-before → method → hypothesis-after → rollback → experiment-end
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

    // ── Tests: estimate and analysis ──────────────────────────

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
        let executor = MockExecutor::always_succeed();
        let controls = ControlRegistry::new();

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
        let executor = MockExecutor::always_succeed();
        let controls = ControlRegistry::new();

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert!(journal.analysis.is_some());
        // Estimate: recovered, Actual: completed (recovered) → accuracy 1.0
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
        let executor = MockExecutor::always_succeed();
        let controls = ControlRegistry::new();

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert!(journal.analysis.is_none());
    }

    // ── Tests: journal metadata ───────────────────────────────

    #[test]
    fn journal_has_correct_title() {
        let exp = minimal_experiment();
        let executor = MockExecutor::always_succeed();
        let controls = ControlRegistry::new();

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.experiment_title, "Test experiment");
    }

    #[test]
    fn journal_has_valid_timestamps() {
        let exp = minimal_experiment();
        let executor = MockExecutor::always_succeed();
        let controls = ControlRegistry::new();

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert!(journal.started_at_ns > 0);
        assert!(journal.ended_at_ns >= journal.started_at_ns);
    }

    #[test]
    fn journal_has_uuid_experiment_id() {
        let exp = minimal_experiment();
        let executor = MockExecutor::always_succeed();
        let controls = ControlRegistry::new();

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        // UUID v4 format: 8-4-4-4-12 hex chars
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
        let executor = MockExecutor::always_succeed();
        let controls = ControlRegistry::new();

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert!(journal.regulatory.is_some());
    }

    // ── Tests: abort with rollback ────────────────────────────

    #[test]
    fn aborted_experiment_runs_rollbacks_on_deviation_strategy() {
        let mut exp = experiment_with_hypothesis();
        exp.rollbacks = vec![test_action("cleanup")];

        // Executor returns 500, hypothesis fails → abort
        // But after abort, rollbacks should still execute since abort is a "deviation"
        struct AbortThenSucceedExecutor {
            call_count: Arc<AtomicUsize>,
        }
        impl ActivityExecutor for AbortThenSucceedExecutor {
            fn execute(&self, _activity: &Activity) -> ActivityOutcome {
                let count = self.call_count.fetch_add(1, Ordering::Relaxed);
                // First call is hypothesis probe → fail with 500
                // Second call (if any) is rollback → succeed
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

        let executor = AbortThenSucceedExecutor {
            call_count: Arc::new(AtomicUsize::new(0)),
        };
        let controls = ControlRegistry::new();

        let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

        assert_eq!(journal.status, ExperimentStatus::Aborted);
        // OnDeviation strategy: abort counts as deviated, so rollbacks run
        assert_eq!(journal.rollback_results.len(), 1);
    }
}
