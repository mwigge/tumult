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
        ..RunConfig::default()
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
        ..RunConfig::default()
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
fn mid_method_cancellation_marks_interrupted_never_completed() {
    // Cancelling mid-method (SIGINT during step-1) breaks the foreground
    // loop. The run must end Interrupted — never Completed — and the fault
    // injected by step-1 must be rolled back.
    struct CancelOnFirstCall {
        token: CancellationToken,
        fired: Arc<AtomicBool>,
    }
    impl ActivityExecutor for CancelOnFirstCall {
        fn execute(&self, _activity: &Activity) -> ActivityOutcome {
            if !self.fired.swap(true, Ordering::SeqCst) {
                self.token.cancel();
            }
            ActivityOutcome {
                success: true,
                output: None,
                error: None,
                duration_ms: 5,
            }
        }
    }

    let mut exp = minimal_experiment();
    exp.method = vec![test_action("step-1"), test_action("step-2")];
    exp.rollbacks = vec![test_action("rollback-1")];

    let token = CancellationToken::new();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(CancelOnFirstCall {
        token: token.clone(),
        fired: Arc::new(AtomicBool::new(false)),
    });
    let controls = Arc::new(ControlRegistry::new());

    let config = RunConfig {
        rollback_strategy: RollbackStrategy::OnDeviation,
        cancellation_token: Some(token),
        parent_context: None,
        load_executor: None,
        ..RunConfig::default()
    };

    let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Interrupted);
    assert_eq!(
        journal.method_results.len(),
        1,
        "step-2 must be skipped once the token fires"
    );
    assert_eq!(
        journal.rollback_results.len(),
        1,
        "the fault injected before cancellation must be rolled back"
    );
}

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
        ..RunConfig::default()
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
        ..RunConfig::default()
    };

    let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Completed);
}

// -- Tests: failed rollback handling

#[test]
fn failed_run_after_fault_started_rolls_back_under_default_strategy() {
    // A method step failure marks the run Failed (not Deviated); the default
    // OnDeviation strategy must still roll back because a fault-injecting
    // action already ran.
    struct FailSecondAction;
    impl ActivityExecutor for FailSecondAction {
        fn execute(&self, activity: &Activity) -> ActivityOutcome {
            if activity.name == "inject-b" {
                ActivityOutcome {
                    success: false,
                    output: None,
                    error: Some("injection failed".into()),
                    duration_ms: 5,
                }
            } else {
                ActivityOutcome {
                    success: true,
                    output: None,
                    error: None,
                    duration_ms: 5,
                }
            }
        }
    }

    let mut exp = minimal_experiment();
    exp.method = vec![test_action("inject-a"), test_action("inject-b")];
    exp.rollbacks = vec![test_action("rollback-1")];
    let executor: Arc<dyn ActivityExecutor> = Arc::new(FailSecondAction);
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Failed);
    assert_eq!(
        journal.rollback_results.len(),
        1,
        "failure after a fault started must trigger rollback under OnDeviation"
    );
}

#[test]
fn failed_run_without_started_fault_skips_rollback_under_default_strategy() {
    // A method containing only a (failing) observation probe never injected a
    // fault, so there is nothing to clean up.
    struct FailProbesExecutor;
    impl ActivityExecutor for FailProbesExecutor {
        fn execute(&self, activity: &Activity) -> ActivityOutcome {
            let is_probe = activity.activity_type == ActivityType::Probe;
            ActivityOutcome {
                success: !is_probe,
                output: None,
                error: if is_probe {
                    Some("probe failed".into())
                } else {
                    None
                },
                duration_ms: 5,
            }
        }
    }

    let mut exp = minimal_experiment();
    exp.method = vec![test_probe("observe-only")];
    exp.rollbacks = vec![test_action("rollback-1")];
    let executor: Arc<dyn ActivityExecutor> = Arc::new(FailProbesExecutor);
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Failed);
    assert!(
        journal.rollback_results.is_empty(),
        "no fault started → no rollback under OnDeviation"
    );
}

#[test]
fn foreground_provider_panic_is_contained_and_rolled_back() {
    // A panicking foreground provider must not unwind out of the run: the
    // activity is recorded as Failed (with panic info), the run ends Failed,
    // and rollbacks still execute per strategy.
    struct PanicForegroundExecutor {
        calls: Arc<AtomicUsize>,
    }
    impl ActivityExecutor for PanicForegroundExecutor {
        fn execute(&self, activity: &Activity) -> ActivityOutcome {
            self.calls.fetch_add(1, Ordering::Relaxed);
            assert!(
                activity.name != "fg-panic",
                "deliberate panic in foreground provider"
            );
            ActivityOutcome {
                success: true,
                output: None,
                error: None,
                duration_ms: 5,
            }
        }
    }

    let mut exp = minimal_experiment();
    exp.method = vec![test_action("fg-panic")];
    exp.rollbacks = vec![test_action("rollback-1")];

    let panicking = PanicForegroundExecutor {
        calls: Arc::new(AtomicUsize::new(0)),
    };
    let calls = panicking.calls.clone();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(panicking);
    let controls = Arc::new(ControlRegistry::new());

    // The panic must not propagate out of the run.
    let journal = run_experiment(&exp, &executor, &controls, &default_config()).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Failed);
    let fg = &journal.method_results[0];
    assert_eq!(fg.status, ActivityStatus::Failed);
    assert!(
        fg.error.as_deref().unwrap_or_default().contains("panicked"),
        "panic info must be recorded in the journal: {:?}",
        fg.error
    );
    assert_eq!(
        journal.rollback_results.len(),
        1,
        "rollback must run after the panic-failed injection"
    );
    assert_eq!(
        calls.load(Ordering::Relaxed),
        2,
        "the rollback activity must execute after the panic"
    );
}

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
        ..RunConfig::default()
    };

    let journal = run_experiment(&exp, &executor, &controls, &config).unwrap();

    assert_eq!(journal.status, ExperimentStatus::Completed);
    assert_eq!(journal.rollback_results.len(), 2);
    assert_eq!(journal.rollback_failures, 2);
}
