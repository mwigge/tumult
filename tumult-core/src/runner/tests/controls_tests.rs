//! Controls lifecycle event tests.

use super::*;
use crate::controls::ControlRegistry;

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
        ..RunConfig::default()
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
        ..RunConfig::default()
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
