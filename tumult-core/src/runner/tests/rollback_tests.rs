//! Rollback execution, abort-with-rollback, cancellation token, and
//! failed-rollback handling tests.

use super::*;
use crate::controls::ControlRegistry;
use std::sync::atomic::AtomicBool;

// -- Tests: rollback execution

#[test]
fn rollbacks_execute_on_deviation_with_default_strategy() {
    // Same probe-flips-after-method-runs pattern as
    // `hypothesis_after_fail_marks_deviated`: the hypothesis-after probe
    // observes "500" and fails, marking the experiment Deviated and
    // triggering rollbacks under the default `OnDeviation` strategy.
    struct DeviatingExecutor {
        method_ran: Arc<AtomicBool>,
    }
    impl ActivityExecutor for DeviatingExecutor {
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

    let mut exp = experiment_with_hypothesis();
    exp.rollbacks = vec![test_action("rollback-1")];
    let executor: Arc<dyn ActivityExecutor> = Arc::new(DeviatingExecutor {
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
