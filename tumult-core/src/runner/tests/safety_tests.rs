//! Regression tests for safety boundaries found in the functional review.
use super::*;
use crate::controls::ControlRegistry;

#[test]
fn failed_probe_with_matching_output_never_passes() {
    let probe = test_probe("health");
    assert!(!super::super::activity::probe_outcome_ok(
        &probe,
        false,
        Some("200")
    ));
}

#[test]
fn cancellation_during_pause_never_dispatches_action() {
    let mut exp = minimal_experiment();
    exp.method[0].pause_before_s = Some(0.1);
    let mock = MockExecutor::always_succeed();
    let calls = mock.call_count.clone();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(mock);
    let token = CancellationToken::new();
    let cancel = token.clone();
    let handle = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        cancel.cancel();
    });
    let journal = run_experiment(
        &exp,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig {
            cancellation_token: Some(token),
            ..RunConfig::default()
        },
    )
    .unwrap();
    handle.join().unwrap();
    assert_eq!(journal.status, ExperimentStatus::Interrupted);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "paused fault must not start after stop"
    );
}

#[test]
fn cancellation_blocks_background_gate_waiters() {
    struct CancelFirst {
        token: CancellationToken,
        calls: Arc<AtomicUsize>,
    }
    impl ActivityExecutor for CancelFirst {
        fn execute(&self, _: &Activity) -> ActivityOutcome {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.token.cancel();
            ActivityOutcome {
                success: true,
                output: None,
                error: None,
                duration_ms: 0,
            }
        }
    }
    let token = CancellationToken::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let executor: Arc<dyn ActivityExecutor> = Arc::new(CancelFirst {
        token: token.clone(),
        calls: calls.clone(),
    });
    let mut exp = minimal_experiment();
    exp.method = (0..8)
        .map(|i| test_action_background(&format!("fault-{i}")))
        .collect();
    run_experiment(
        &exp,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig {
            cancellation_token: Some(token),
            max_concurrent_faults: Some(1),
            ..RunConfig::default()
        },
    )
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn cancellation_during_post_phase_interrupts_and_rolls_back() {
    struct CancelPost {
        token: CancellationToken,
        caller: std::thread::ThreadId,
        probes: AtomicUsize,
    }
    impl ActivityExecutor for CancelPost {
        fn execute(&self, activity: &Activity) -> ActivityOutcome {
            // The first foreground probe is hypothesis-before; the next is post.
            if activity.activity_type == ActivityType::Probe
                && std::thread::current().id() == self.caller
                && self.probes.fetch_add(1, Ordering::SeqCst) > 0
            {
                self.token.cancel();
            }
            ActivityOutcome {
                success: true,
                output: Some("200".into()),
                error: None,
                duration_ms: 0,
            }
        }
    }
    let token = CancellationToken::new();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(CancelPost {
        token: token.clone(),
        caller: std::thread::current().id(),
        probes: AtomicUsize::new(0),
    });
    let mut exp = experiment_with_hypothesis();
    exp.rollbacks.push(test_action("cleanup"));
    let journal = run_experiment_with_sampling(
        &exp,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig {
            cancellation_token: Some(token),
            ..RunConfig::default()
        },
        &fast_sampling(),
    )
    .unwrap();
    assert_eq!(journal.status, ExperimentStatus::Interrupted);
    assert_eq!(journal.rollback_results.len(), 1);
}

#[test]
fn baseline_only_runs_probes_without_controls_faults_load_or_rollbacks() {
    let mut exp = experiment_with_hypothesis();
    exp.rollbacks.push(test_action("cleanup"));
    exp.load = Some(LoadConfig {
        tool: LoadTool::K6,
        script: "unused.js".into(),
        vus: None,
        duration_s: None,
        thresholds: HashMap::new(),
    });
    let mock = MockExecutor::always_succeed();
    let calls = mock.call_count.clone();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(mock);
    let (recorder, events) = EventRecorder::new();
    let mut controls = ControlRegistry::new();
    controls.register(Box::new(recorder));
    let journal = run_experiment(
        &exp,
        &executor,
        &Arc::new(controls),
        &RunConfig {
            baseline_mode: BaselineMode::Only,
            rollback_strategy: RollbackStrategy::Always,
            ..RunConfig::default()
        },
    )
    .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(events.lock().unwrap().is_empty());
    assert!(
        journal.method_results.is_empty()
            && journal.rollback_results.is_empty()
            && journal.load_result.is_none()
    );
    assert!(journal
        .experiment_title
        .ends_with("[baseline-only observation]"));
}

#[test]
fn baseline_only_requires_probes() {
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor::always_succeed());
    assert!(run_experiment(
        &minimal_experiment(),
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig {
            baseline_mode: BaselineMode::Only,
            ..RunConfig::default()
        }
    )
    .is_err());
}

#[test]
fn declared_load_without_executor_fails_before_faults() {
    let mut exp = minimal_experiment();
    exp.load = Some(LoadConfig {
        tool: LoadTool::K6,
        script: "unused.js".into(),
        vus: None,
        duration_s: None,
        thresholds: HashMap::new(),
    });
    let mock = MockExecutor::always_succeed();
    let calls = mock.call_count.clone();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(mock);
    assert!(run_experiment(
        &exp,
        &executor,
        &Arc::new(ControlRegistry::new()),
        &RunConfig::default()
    )
    .is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn cancellation_from_before_activity_control_prevents_dispatch() {
    struct StopControl(CancellationToken);
    impl crate::controls::ControlHandler for StopControl {
        fn name(&self) -> &'static str {
            "stop-before-dispatch"
        }
        fn on_event(&self, event: &LifecycleEvent) {
            if matches!(event, LifecycleEvent::BeforeActivity { .. }) {
                self.0.cancel();
            }
        }
    }
    let token = CancellationToken::new();
    let mut controls = ControlRegistry::new();
    controls.register(Box::new(StopControl(token.clone())));
    let mock = MockExecutor::always_succeed();
    let calls = mock.call_count.clone();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(mock);
    let journal = run_experiment(
        &minimal_experiment(),
        &executor,
        &Arc::new(controls),
        &RunConfig {
            cancellation_token: Some(token),
            ..RunConfig::default()
        },
    )
    .unwrap();
    assert_eq!(journal.status, ExperimentStatus::Interrupted);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn panicking_rollback_does_not_skip_remaining_cleanup() {
    struct PanicCleanup(Arc<std::sync::Mutex<Vec<String>>>);
    impl ActivityExecutor for PanicCleanup {
        fn execute(&self, activity: &Activity) -> ActivityOutcome {
            self.0.lock().unwrap().push(activity.name.clone());
            assert_ne!(activity.name, "panic-cleanup", "test rollback panic");
            ActivityOutcome {
                success: true,
                output: None,
                error: None,
                duration_ms: 0,
            }
        }
    }
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    let executor: Arc<dyn ActivityExecutor> = Arc::new(PanicCleanup(calls.clone()));
    let mut experiment = minimal_experiment();
    experiment.rollbacks = vec![
        test_action("panic-cleanup"),
        test_action("remaining-cleanup"),
    ];
    let results = run_orphan_rollback(&experiment, &executor, &Arc::new(ControlRegistry::new()));
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].status, ActivityStatus::Failed);
    assert_eq!(results[1].status, ActivityStatus::Succeeded);
    assert_eq!(
        calls.lock().unwrap().as_slice(),
        ["panic-cleanup", "remaining-cleanup"]
    );
}
