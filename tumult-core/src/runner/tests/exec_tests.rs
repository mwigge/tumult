//! Basic execution, hypothesis evaluation, estimate/analysis, and journal
//! metadata tests.

use super::*;
use crate::controls::ControlRegistry;
use std::sync::atomic::AtomicBool;

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
    // Probes report healthy ("200") until the method action runs, then
    // report unhealthy ("500"). The hypothesis-before probe (which runs
    // before the method) sees "200" and passes; the hypothesis-after
    // probe (which runs after the method) sees "500" and fails,
    // marking the experiment Deviated. During-phase probes run
    // concurrently with the method and may observe either value, but
    // during_result doesn't gate `status` so that race doesn't matter
    // for this test.
    struct AlternatingExecutor {
        method_ran: Arc<AtomicBool>,
    }
    impl ActivityExecutor for AlternatingExecutor {
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

    let exp = experiment_with_hypothesis();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(AlternatingExecutor {
        method_ran: Arc::new(AtomicBool::new(false)),
    });
    let controls = Arc::new(ControlRegistry::new());

    // Fast sampling: probes stay unhealthy after the method, so the
    // post-phase recovery loop would otherwise run to its default timeout.
    let journal = run_experiment_with_sampling(
        &exp,
        &executor,
        &controls,
        &default_config(),
        &fast_sampling(),
    )
    .unwrap();

    assert_eq!(journal.status, ExperimentStatus::Deviated);
    assert!(journal.steady_state_after.is_some());
    assert!(!journal.steady_state_after.as_ref().unwrap().met);
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
