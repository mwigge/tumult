//! Integration tests for the experiment runner.
//!
//! These tests exercise the full experiment lifecycle using mock plugins,
//! validating the five-phase execution, hypothesis evaluation, rollbacks,
//! background activities, and estimate accuracy.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use indexmap::IndexMap;
use tumult_core::controls::{ControlHandler, ControlRegistry, LifecycleEvent};
use tumult_core::execution::RollbackStrategy;
use tumult_core::runner::{run_experiment, ActivityExecutor, ActivityOutcome, RunConfig};
use tumult_core::types::*;

// ── Mock Plugin Executor ──────────────────────────────────────

/// A configurable mock executor that simulates plugin behavior.
struct MockPlugin {
    /// Map from activity name to (success, output) pairs.
    responses: HashMap<String, (bool, Option<String>)>,
    /// Default response for unknown activities.
    default_success: bool,
    default_output: Option<String>,
    /// Track execution order.
    execution_log: Arc<std::sync::Mutex<Vec<String>>>,
}

impl MockPlugin {
    fn new() -> Self {
        Self {
            responses: HashMap::new(),
            default_success: true,
            default_output: Some("200".into()),
            execution_log: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn on(mut self, name: &str, success: bool, output: Option<&str>) -> Self {
        self.responses
            .insert(name.into(), (success, output.map(String::from)));
        self
    }

    fn default_output(mut self, output: &str) -> Self {
        self.default_output = Some(output.into());
        self
    }

    fn default_fail(mut self) -> Self {
        self.default_success = false;
        self.default_output = None;
        self
    }

    fn log(&self) -> Vec<String> {
        self.execution_log.lock().unwrap().clone()
    }
}

impl ActivityExecutor for MockPlugin {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        self.execution_log
            .lock()
            .unwrap()
            .push(activity.name.clone());

        if let Some((success, output)) = self.responses.get(&activity.name) {
            ActivityOutcome {
                success: *success,
                output: output.clone(),
                error: if *success {
                    None
                } else {
                    Some(format!("{} failed", activity.name))
                },
                duration_ms: 10,
            }
        } else {
            ActivityOutcome {
                success: self.default_success,
                output: self.default_output.clone(),
                error: if self.default_success {
                    None
                } else {
                    Some("default failure".into())
                },
                duration_ms: 10,
            }
        }
    }
}

// ── Event recorder ────────────────────────────────────────────

struct EventLog {
    events: Arc<std::sync::Mutex<Vec<LifecycleEvent>>>,
}

impl EventLog {
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

impl ControlHandler for EventLog {
    fn name(&self) -> &str {
        "event-log"
    }
    fn on_event(&self, event: &LifecycleEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

// ── Test helpers ──────────────────────────────────────────────

fn action(name: &str) -> Activity {
    Activity {
        name: name.into(),
        activity_type: ActivityType::Action,
        provider: Provider::Native {
            plugin: "mock".into(),
            function: "noop".into(),
            arguments: HashMap::new(),
        },
        tolerance: None,
        pause_before_s: None,
        pause_after_s: None,
        background: false,
    }
}

fn background_action(name: &str) -> Activity {
    Activity {
        background: true,
        ..action(name)
    }
}

fn probe_with_tolerance(name: &str, expected: serde_json::Value) -> Activity {
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
        tolerance: Some(Tolerance::Exact { value: expected }),
        pause_before_s: None,
        pause_after_s: None,
        background: false,
    }
}

fn hypothesis(title: &str, probes: Vec<Activity>) -> Hypothesis {
    Hypothesis {
        title: title.into(),
        probes,
    }
}

fn experiment_builder() -> Experiment {
    Experiment {
        version: "v1".into(),
        title: "Integration test experiment".into(),
        description: Some("Tests the full five-phase lifecycle".into()),
        tags: vec!["integration".into(), "test".into()],
        configuration: IndexMap::new(),
        secrets: IndexMap::new(),
        controls: vec![],
        steady_state_hypothesis: None,
        method: vec![action("inject-fault")],
        rollbacks: vec![],
        estimate: None,
        baseline: None,
        load: None,
        regulatory: None,
    }
}

// ═══════════════════════════════════════════════════════════════
// Task 74: Full experiment run with mock plugin → journal with all phases
// ═══════════════════════════════════════════════════════════════

#[test]
fn full_experiment_run_produces_complete_journal() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "System is healthy",
        vec![probe_with_tolerance(
            "health-check",
            serde_json::Value::Number(200.into()),
        )],
    ));
    exp.method = vec![action("inject-fault"), action("wait-for-propagation")];
    exp.rollbacks = vec![action("cleanup-fault")];
    exp.estimate = Some(Estimate {
        expected_outcome: ExpectedOutcome::Recovered,
        expected_recovery_s: Some(10.0),
        expected_degradation: Some(DegradationLevel::Minor),
        expected_data_loss: Some(false),
        confidence: Some(Confidence::High),
        rationale: Some("Tested before".into()),
        prior_runs: Some(3),
    });
    exp.regulatory = Some(RegulatoryMapping {
        frameworks: vec!["DORA".into()],
        requirements: vec![RegulatoryRequirement {
            id: "DORA-Art24".into(),
            description: "ICT resilience testing".into(),
            evidence: "Recovery within RTO".into(),
        }],
    });

    let plugin = MockPlugin::new().default_output("200");
    let mut controls = ControlRegistry::new();
    let (logger, events) = EventLog::new();
    controls.register(Box::new(logger));

    let config = RunConfig {
        rollback_strategy: RollbackStrategy::Always,
        cancellation_token: None,
    };

    let journal = run_experiment(&exp, &plugin, &controls, &config).unwrap();

    // Journal completeness checks
    assert_eq!(journal.experiment_title, "Integration test experiment");
    assert!(!journal.experiment_id.is_empty());
    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert!(journal.started_at_ns > 0);
    assert!(journal.ended_at_ns >= journal.started_at_ns);
    assert!(journal.duration_ms < 10_000); // Should be fast with mocks

    // Hypothesis results
    assert!(journal.steady_state_before.is_some());
    assert!(journal.steady_state_before.as_ref().unwrap().met);
    assert!(journal.steady_state_after.is_some());
    assert!(journal.steady_state_after.as_ref().unwrap().met);

    // Method results
    assert_eq!(journal.method_results.len(), 2);
    assert_eq!(journal.method_results[0].name, "inject-fault");
    assert_eq!(journal.method_results[1].name, "wait-for-propagation");

    // Rollback results (Always strategy)
    assert_eq!(journal.rollback_results.len(), 1);
    assert_eq!(journal.rollback_results[0].name, "cleanup-fault");

    // Estimate preserved
    assert!(journal.estimate.is_some());
    assert_eq!(
        journal.estimate.as_ref().unwrap().expected_outcome,
        ExpectedOutcome::Recovered
    );

    // Analysis computed
    assert!(journal.analysis.is_some());
    assert_eq!(
        journal.analysis.as_ref().unwrap().estimate_accuracy,
        Some(1.0)
    );

    // Regulatory preserved
    assert!(journal.regulatory.is_some());

    // Lifecycle events emitted in correct order
    let events = events.lock().unwrap();
    assert!(!events.is_empty());
    assert_eq!(events[0], LifecycleEvent::BeforeExperiment);
    assert_eq!(*events.last().unwrap(), LifecycleEvent::AfterExperiment);

    // Execution log shows all activities ran
    let log = plugin.log();
    assert!(log.contains(&"health-check".to_string())); // hypothesis before
    assert!(log.contains(&"inject-fault".to_string()));
    assert!(log.contains(&"wait-for-propagation".to_string()));
    assert!(log.contains(&"cleanup-fault".to_string()));
}

#[test]
fn journal_serializes_to_toon_and_back() {
    let exp = experiment_builder();
    let plugin = MockPlugin::new();
    let controls = ControlRegistry::new();

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    // Round-trip through TOON
    let toon = tumult_core::journal::encode_journal(&journal).unwrap();
    assert!(!toon.is_empty());

    let decoded: Journal = toon_format::decode_default(&toon).unwrap();
    assert_eq!(decoded.experiment_title, journal.experiment_title);
    assert_eq!(decoded.status, journal.status);
    assert_eq!(decoded.method_results.len(), journal.method_results.len());
}

// ═══════════════════════════════════════════════════════════════
// Task 75: Baselined hypothesis — derive then compare
// ═══════════════════════════════════════════════════════════════

#[test]
fn baselined_hypothesis_with_range_tolerance() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "Latency within range",
        vec![Activity {
            name: "latency-probe".into(),
            activity_type: ActivityType::Probe,
            provider: Provider::Http {
                method: HttpMethod::Get,
                url: "http://localhost/metrics".into(),
                headers: HashMap::new(),
                body: None,
                timeout_s: Some(5.0),
            },
            // Simulating derived tolerance: latency between 20-80ms
            tolerance: Some(Tolerance::Range {
                from: 20.0,
                to: 80.0,
            }),
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        }],
    ));

    // Probe returns 45.0 (within range)
    let plugin = MockPlugin::new()
        .on("latency-probe", true, Some("45.0"))
        .default_output("200");
    let controls = ControlRegistry::new();

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert!(journal.steady_state_before.as_ref().unwrap().met);
    assert!(journal.steady_state_after.as_ref().unwrap().met);
}

#[test]
fn baselined_hypothesis_fails_when_outside_range() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "Latency within range",
        vec![Activity {
            name: "latency-probe".into(),
            activity_type: ActivityType::Probe,
            provider: Provider::Http {
                method: HttpMethod::Get,
                url: "http://localhost/metrics".into(),
                headers: HashMap::new(),
                body: None,
                timeout_s: Some(5.0),
            },
            tolerance: Some(Tolerance::Range {
                from: 20.0,
                to: 80.0,
            }),
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        }],
    ));

    // Probe returns 150.0 (outside range) — hypothesis fails before method
    let plugin = MockPlugin::new().on("latency-probe", true, Some("150.0"));
    let controls = ControlRegistry::new();

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Aborted);
    assert!(!journal.steady_state_before.as_ref().unwrap().met);
    assert!(journal.method_results.is_empty());
}

// ═══════════════════════════════════════════════════════════════
// Task 76: Hypothesis failure → abort → rollbacks
// ═══════════════════════════════════════════════════════════════

#[test]
fn hypothesis_failure_aborts_and_runs_rollbacks() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "System is healthy",
        vec![probe_with_tolerance(
            "health-check",
            serde_json::Value::Number(200.into()),
        )],
    ));
    exp.rollbacks = vec![action("emergency-cleanup"), action("notify-ops")];

    // Health check returns 503 → hypothesis fails → abort
    let plugin = MockPlugin::new()
        .on("health-check", true, Some("503"))
        .on("emergency-cleanup", true, Some("ok"))
        .on("notify-ops", true, Some("ok"));

    let mut controls = ControlRegistry::new();
    let (logger, events) = EventLog::new();
    controls.register(Box::new(logger));

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    // Status should be aborted
    assert_eq!(journal.status, ExperimentStatus::Aborted);

    // Hypothesis before should have failed
    assert!(!journal.steady_state_before.as_ref().unwrap().met);

    // Method should NOT have executed
    assert!(journal.method_results.is_empty());

    // Rollbacks SHOULD have executed (abort is treated as deviation)
    assert_eq!(journal.rollback_results.len(), 2);
    assert_eq!(journal.rollback_results[0].name, "emergency-cleanup");
    assert_eq!(journal.rollback_results[1].name, "notify-ops");

    // No hypothesis after (never reached)
    assert!(journal.steady_state_after.is_none());

    // Verify lifecycle events
    let events = events.lock().unwrap();
    assert!(events.contains(&LifecycleEvent::BeforeRollback));
    assert!(events.contains(&LifecycleEvent::AfterRollback));
    // Method events should NOT be present
    assert!(!events.contains(&LifecycleEvent::BeforeMethod));
}

#[test]
fn hypothesis_after_failure_causes_deviation_with_rollback() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "System is healthy",
        vec![probe_with_tolerance(
            "health-check",
            serde_json::Value::Number(200.into()),
        )],
    ));
    exp.rollbacks = vec![action("rollback-action")];

    // Use call-count based executor to return different results
    struct PhaseAwareExecutor {
        call_count: AtomicUsize,
    }
    impl ActivityExecutor for PhaseAwareExecutor {
        fn execute(&self, _activity: &Activity) -> ActivityOutcome {
            let count = self.call_count.fetch_add(1, Ordering::Relaxed);
            match count {
                0 => ActivityOutcome {
                    // Hypothesis before: pass
                    success: true,
                    output: Some("200".into()),
                    error: None,
                    duration_ms: 10,
                },
                1 => ActivityOutcome {
                    // Method: succeed
                    success: true,
                    output: Some("fault injected".into()),
                    error: None,
                    duration_ms: 100,
                },
                2 => ActivityOutcome {
                    // Hypothesis after: FAIL (system degraded)
                    success: true,
                    output: Some("503".into()),
                    error: None,
                    duration_ms: 10,
                },
                _ => ActivityOutcome {
                    // Rollback: succeed
                    success: true,
                    output: Some("rolled back".into()),
                    error: None,
                    duration_ms: 10,
                },
            }
        }
    }

    let executor = PhaseAwareExecutor {
        call_count: AtomicUsize::new(0),
    };
    let controls = ControlRegistry::new();

    let journal = run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    assert!(journal.steady_state_before.as_ref().unwrap().met);
    assert!(!journal.steady_state_after.as_ref().unwrap().met);
    assert_eq!(journal.method_results.len(), 1);
    // OnDeviation strategy: rollbacks should execute
    assert_eq!(journal.rollback_results.len(), 1);
}

// ═══════════════════════════════════════════════════════════════
// Task 77: Background activities execute concurrently
// ═══════════════════════════════════════════════════════════════

#[test]
fn background_and_sequential_activities_all_execute() {
    // Note: The current runner executes all activities sequentially
    // (background support requires async). This test verifies that
    // background-flagged activities still execute in the method.
    let mut exp = experiment_builder();
    exp.method = vec![
        action("sequential-1"),
        background_action("background-1"),
        action("sequential-2"),
        background_action("background-2"),
    ];

    let plugin = MockPlugin::new();
    let controls = ControlRegistry::new();

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert_eq!(journal.method_results.len(), 4);

    // All activities should have executed
    let log = plugin.log();
    assert_eq!(log.len(), 4);
    assert!(log.contains(&"sequential-1".to_string()));
    assert!(log.contains(&"background-1".to_string()));
    assert!(log.contains(&"sequential-2".to_string()));
    assert!(log.contains(&"background-2".to_string()));
}

// ═══════════════════════════════════════════════════════════════
// Task 78: Estimate vs actual accuracy calculation
// ═══════════════════════════════════════════════════════════════

#[test]
fn estimate_accuracy_correct_when_prediction_matches() {
    let mut exp = experiment_builder();
    exp.estimate = Some(Estimate {
        expected_outcome: ExpectedOutcome::Recovered,
        expected_recovery_s: Some(10.0),
        expected_degradation: Some(DegradationLevel::Minor),
        expected_data_loss: Some(false),
        confidence: Some(Confidence::High),
        rationale: None,
        prior_runs: Some(5),
    });

    let plugin = MockPlugin::new();
    let controls = ControlRegistry::new();

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    // Estimated: Recovered, Actual: Completed (recovered) → accuracy 1.0
    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert!(journal.analysis.is_some());
    let analysis = journal.analysis.unwrap();
    assert_eq!(analysis.estimate_accuracy, Some(1.0));
    assert_eq!(analysis.resilience_score, Some(1.0));
}

#[test]
fn estimate_accuracy_zero_when_prediction_wrong() {
    let mut exp = experiment_builder();
    exp.estimate = Some(Estimate {
        expected_outcome: ExpectedOutcome::Recovered,
        expected_recovery_s: Some(10.0),
        expected_degradation: None,
        expected_data_loss: None,
        confidence: Some(Confidence::Medium),
        rationale: None,
        prior_runs: None,
    });

    // Make the experiment fail
    let plugin = MockPlugin::new().default_fail();
    let controls = ControlRegistry::new();

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    // Estimated: Recovered, Actual: Failed → accuracy 0.0
    assert_eq!(journal.status, ExperimentStatus::Failed);
    assert!(journal.analysis.is_some());
    let analysis = journal.analysis.unwrap();
    assert_eq!(analysis.estimate_accuracy, Some(0.0));
    assert_eq!(analysis.resilience_score, Some(0.0));
}

#[test]
fn estimate_accuracy_when_deviated_matches_estimate() {
    let mut exp = experiment_builder();
    exp.estimate = Some(Estimate {
        expected_outcome: ExpectedOutcome::Deviated,
        expected_recovery_s: None,
        expected_degradation: Some(DegradationLevel::Severe),
        expected_data_loss: None,
        confidence: Some(Confidence::Low),
        rationale: None,
        prior_runs: None,
    });
    exp.steady_state_hypothesis = Some(hypothesis(
        "System responds",
        vec![probe_with_tolerance(
            "health-check",
            serde_json::Value::Number(200.into()),
        )],
    ));

    // Hypothesis before passes, method succeeds, hypothesis after fails → deviated
    struct DeviationExecutor {
        call_count: AtomicUsize,
    }
    impl ActivityExecutor for DeviationExecutor {
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

    let executor = DeviationExecutor {
        call_count: AtomicUsize::new(0),
    };
    let controls = ControlRegistry::new();

    let journal = run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    // Estimated: Deviated, Actual: Deviated → not "recovered"
    // Both estimate and actual are non-recovered → accuracy 1.0
    assert_eq!(journal.status, ExperimentStatus::Deviated);
    assert!(journal.analysis.is_some());
    let analysis = journal.analysis.unwrap();
    assert_eq!(analysis.estimate_accuracy, Some(1.0));
}

#[test]
fn no_analysis_without_estimate() {
    let exp = experiment_builder();
    let plugin = MockPlugin::new();
    let controls = ControlRegistry::new();

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    assert!(journal.analysis.is_none());
}

// ═══════════════════════════════════════════════════════════════
// Additional integration: regex tolerance
// ═══════════════════════════════════════════════════════════════

#[test]
fn regex_tolerance_in_hypothesis() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "Status matches pattern",
        vec![Activity {
            name: "status-probe".into(),
            activity_type: ActivityType::Probe,
            provider: Provider::Http {
                method: HttpMethod::Get,
                url: "http://localhost/status".into(),
                headers: HashMap::new(),
                body: None,
                timeout_s: Some(5.0),
            },
            tolerance: Some(Tolerance::Regex {
                pattern: "^OK.*".into(),
            }),
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        }],
    ));

    let plugin = MockPlugin::new()
        .on("status-probe", true, Some("\"OK: all systems go\""))
        .default_output("200");
    let controls = ControlRegistry::new();

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert!(journal.steady_state_before.as_ref().unwrap().met);
}

// ═══════════════════════════════════════════════════════════════
// Additional integration: multiple hypothesis probes
// ═══════════════════════════════════════════════════════════════

#[test]
fn multiple_hypothesis_probes_all_must_pass() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "All services healthy",
        vec![
            probe_with_tolerance("api-health", serde_json::Value::Number(200.into())),
            probe_with_tolerance("db-health", serde_json::Value::Number(200.into())),
            probe_with_tolerance("cache-health", serde_json::Value::Number(200.into())),
        ],
    ));

    // All probes pass
    let plugin = MockPlugin::new().default_output("200");
    let controls = ControlRegistry::new();

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();
    assert_eq!(journal.status, ExperimentStatus::Completed);
}

#[test]
fn one_failing_hypothesis_probe_causes_abort() {
    let mut exp = experiment_builder();
    exp.steady_state_hypothesis = Some(hypothesis(
        "All services healthy",
        vec![
            probe_with_tolerance("api-health", serde_json::Value::Number(200.into())),
            probe_with_tolerance("db-health", serde_json::Value::Number(200.into())),
        ],
    ));

    // db-health returns 503 → one probe fails → hypothesis fails
    let plugin =
        MockPlugin::new()
            .on("api-health", true, Some("200"))
            .on("db-health", true, Some("503"));
    let controls = ControlRegistry::new();

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();
    assert_eq!(journal.status, ExperimentStatus::Aborted);
}
