//! Span-name assertion tests for the experiment runner.
//!
//! These tests verify that the runner emits the correct span names as
//! specified in README.md:
//!   resilience.experiment  (root)
//!   resilience.hypothesis.before
//!   resilience.hypothesis.after
//!   resilience.action
//!   resilience.probe
//!   resilience.rollback

use std::sync::Arc;

use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{run_experiment, ActivityExecutor, RunConfig};
use tumult_core::types::*;

use super::{setup_in_memory_provider, simple_experiment, span_names, MockExecutor};

#[test]
fn runner_emits_resilience_action_span_name() {
    let (provider, exporter, _lock) = setup_in_memory_provider();

    let exp = Experiment {
        title: "action span name test".into(),
        method: vec![Activity {
            name: "inject-fault".into(),
            activity_type: ActivityType::Action,
            ..Default::default()
        }],
        ..Default::default()
    };

    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor);
    let controls = Arc::new(ControlRegistry::new());
    run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    let names = span_names(&exporter);
    assert!(
        names.iter().any(|n| n == "resilience.action"),
        "expected 'resilience.action' span, got: {names:?}"
    );

    let _ = provider.shutdown();
}

#[test]
fn runner_emits_resilience_probe_span_name() {
    let (provider, exporter, _lock) = setup_in_memory_provider();

    let exp = Experiment {
        title: "probe span name test".into(),
        method: vec![Activity {
            name: "dummy-action".into(),
            activity_type: ActivityType::Action,
            ..Default::default()
        }],
        steady_state_hypothesis: Some(Hypothesis {
            title: "healthy".into(),
            probes: vec![Activity {
                name: "health-check".into(),
                activity_type: ActivityType::Probe,
                tolerance: Some(Tolerance::Exact {
                    value: serde_json::Value::Number(200.into()),
                }),
                ..Default::default()
            }],
        }),
        ..Default::default()
    };

    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor);
    let controls = Arc::new(ControlRegistry::new());
    run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    let names = span_names(&exporter);
    assert!(
        names.iter().any(|n| n == "resilience.probe"),
        "expected 'resilience.probe' span, got: {names:?}"
    );

    let _ = provider.shutdown();
}

#[test]
fn runner_emits_resilience_experiment_root_span() {
    let (provider, exporter, _lock) = setup_in_memory_provider();

    let exp = simple_experiment();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor);
    let controls = Arc::new(ControlRegistry::new());
    run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    let names = span_names(&exporter);
    assert!(
        names.iter().any(|n| n == "resilience.experiment"),
        "expected 'resilience.experiment' root span, got: {names:?}"
    );

    let _ = provider.shutdown();
}

#[test]
fn runner_emits_resilience_hypothesis_spans() {
    let (provider, exporter, _lock) = setup_in_memory_provider();

    let exp = simple_experiment();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor);
    let controls = Arc::new(ControlRegistry::new());
    run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    let names = span_names(&exporter);
    assert!(
        names.iter().any(|n| n == "resilience.hypothesis.before"),
        "expected 'resilience.hypothesis.before' span, got: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == "resilience.hypothesis.after"),
        "expected 'resilience.hypothesis.after' span, got: {names:?}"
    );

    let _ = provider.shutdown();
}

#[test]
fn runner_emits_resilience_rollback_span() {
    let (provider, exporter, _lock) = setup_in_memory_provider();

    let mut exp = simple_experiment();
    // Add rollback that will execute (always strategy)
    exp.rollbacks = vec![Activity {
        name: "undo-fault".into(),
        activity_type: ActivityType::Action,
        ..Default::default()
    }];

    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor);
    let controls = Arc::new(ControlRegistry::new());
    let config = RunConfig {
        baseline_mode: tumult_core::runner::BaselineMode::default(),
        rollback_strategy: tumult_core::execution::RollbackStrategy::Always,
        cancellation_token: None,
        parent_context: None,
        load_executor: None,
        max_concurrent_faults: None,
    };
    run_experiment(&exp, &executor, &controls, &config).unwrap();

    let names = span_names(&exporter);
    assert!(
        names.iter().any(|n| n == "resilience.rollback"),
        "expected 'resilience.rollback' span, got: {names:?}"
    );

    let _ = provider.shutdown();
}

/// The root span carries the discipline and the declared target, which is
/// what the lake scores, scopes and gates by.
#[test]
fn runner_emits_test_kind_and_target_on_root_span() {
    let (provider, exporter, _lock) = setup_in_memory_provider();

    let mut exp = simple_experiment();
    exp.kind = TestKind::Dr;
    exp.rto_target_s = Some(30.0);
    exp.target = Some(ExperimentTarget {
        system: "payments-api".into(),
        technology: Some("postgresql".into()),
        environment: Some("staging".into()),
        criticality: None,
    });
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor);
    let controls = Arc::new(ControlRegistry::new());
    run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    let root = exporter
        .get_finished_spans()
        .unwrap_or_default()
        .into_iter()
        .find(|s| s.name == "resilience.experiment")
        .expect("root span");
    let attr = |key: &str| -> Option<String> {
        root.attributes
            .iter()
            .find(|kv| kv.key.as_str() == key)
            .map(|kv| kv.value.as_str().into_owned())
    };
    assert_eq!(attr("resilience.test.kind").as_deref(), Some("dr"));
    assert_eq!(
        attr("resilience.target.system").as_deref(),
        Some("payments-api")
    );
    assert_eq!(
        attr("resilience.target.technology").as_deref(),
        Some("postgresql")
    );
    assert_eq!(
        attr("resilience.target.environment").as_deref(),
        Some("staging")
    );
    assert_eq!(attr("resilience.target.criticality"), None);

    let _ = provider.shutdown();
}
