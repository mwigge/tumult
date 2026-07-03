//! Full experiment run and journal serialization tests (Task 74).

use crate::common::*;

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

    let mock_plugin = MockPlugin::new().default_output("200");
    let execution_log = mock_plugin.execution_log.clone();
    let plugin: Arc<dyn ActivityExecutor> = Arc::new(mock_plugin);
    let mut controls = ControlRegistry::new();
    let (logger, events) = EventLog::new();
    controls.register(Box::new(logger));
    let controls = Arc::new(controls);

    let config = RunConfig {
        rollback_strategy: RollbackStrategy::Always,
        cancellation_token: None,
        parent_context: None,
        load_executor: None,
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
    let log = execution_log.lock().unwrap();
    assert!(log.contains(&"health-check".to_string())); // hypothesis before
    assert!(log.contains(&"inject-fault".to_string()));
    assert!(log.contains(&"wait-for-propagation".to_string()));
    assert!(log.contains(&"cleanup-fault".to_string()));
}

#[test]
fn journal_serializes_to_toon_and_back() {
    let exp = experiment_builder();
    let plugin: Arc<dyn ActivityExecutor> = Arc::new(MockPlugin::new());
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &plugin, &controls, &RunConfig::default()).unwrap();

    // Round-trip through TOON
    let toon = tumult_core::journal::encode_journal(&journal).unwrap();
    assert!(!toon.is_empty());

    let decoded: Journal = toon_format::decode_default(&toon).unwrap();
    assert_eq!(decoded.experiment_title, journal.experiment_title);
    assert_eq!(decoded.status, journal.status);
    assert_eq!(decoded.method_results.len(), journal.method_results.len());
}
