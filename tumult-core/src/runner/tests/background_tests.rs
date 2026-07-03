//! Background task spawning, during-phase sampling, MTTR, and audit-log tests.

use super::*;
use crate::controls::ControlRegistry;

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
