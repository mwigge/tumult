//! Estimate-vs-actual accuracy tests (Task 78).

use std::sync::atomic::{AtomicBool, Ordering};

use crate::common::*;

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

    let plugin: Arc<dyn ActivityExecutor> = Arc::new(MockPlugin::new());
    let controls = Arc::new(ControlRegistry::new());

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
    let plugin: Arc<dyn ActivityExecutor> = Arc::new(MockPlugin::new().default_fail());
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    // Estimated: Recovered, Actual: Failed → accuracy 0.0
    assert_eq!(journal.status, ExperimentStatus::Failed);
    assert!(journal.analysis.is_some());
    let analysis = journal.analysis.unwrap();
    assert_eq!(analysis.estimate_accuracy, Some(0.0));
    assert_eq!(analysis.resilience_score, Some(0.0));
}

struct DeviationExecutor {
    method_ran: AtomicBool,
}
impl ActivityExecutor for DeviationExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        match activity.activity_type {
            ActivityType::Action => {
                self.method_ran.store(true, Ordering::SeqCst);
                ActivityOutcome {
                    success: true,
                    output: None,
                    error: None,
                    duration_ms: 10,
                }
            }
            ActivityType::Probe => {
                let output = if self.method_ran.load(Ordering::SeqCst) {
                    "500"
                } else {
                    "200"
                };
                ActivityOutcome {
                    success: true,
                    output: Some(output.into()),
                    error: None,
                    duration_ms: 10,
                }
            }
        }
    }
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
    let executor: Arc<dyn ActivityExecutor> = Arc::new(DeviationExecutor {
        method_ran: AtomicBool::new(false),
    });
    let controls = Arc::new(ControlRegistry::new());

    // Fast sampling: probes stay unhealthy after the method, so the
    // post-phase recovery loop would otherwise run to its default timeout.
    let journal = run_experiment_with_sampling(
        &exp,
        &executor,
        &controls,
        &RunConfig::default(),
        &fast_sampling(),
    )
    .unwrap();

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
    let plugin: Arc<dyn ActivityExecutor> = Arc::new(MockPlugin::new());
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    assert!(journal.analysis.is_none());
}
