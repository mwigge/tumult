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

use opentelemetry_sdk::trace::{InMemorySpanExporter, SpanData};
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
        version_resolver: None,
        baseline_mode: tumult_core::runner::BaselineMode::default(),
        rollback_strategy: tumult_core::execution::RollbackStrategy::Always,
        cancellation_token: None,
        parent_context: None,
        load_executor: None,
    };
    run_experiment(&exp, &executor, &controls, &config).unwrap();

    let names = span_names(&exporter);
    assert!(
        names.iter().any(|n| n == "resilience.rollback"),
        "expected 'resilience.rollback' span, got: {names:?}"
    );

    let _ = provider.shutdown();
}

/// The root span of the run against `system`. A test in this binary runs
/// without the tracer lock, so the exporter can hold a root that is not
/// this test's.
fn root_span_for(exporter: &InMemorySpanExporter, system: &str) -> SpanData {
    exporter
        .get_finished_spans()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.name == "resilience.experiment")
        .find(|s| {
            s.attributes.iter().any(|kv| {
                kv.key.as_str() == "resilience.target.system" && kv.value.as_str() == system
            })
        })
        .expect("root span")
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
        version: None,
        system: "payments-api".into(),
        technology: Some("postgresql".into()),
        environment: Some("staging".into()),
        criticality: None,
        namespace: None,
    });
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor);
    let controls = Arc::new(ControlRegistry::new());
    run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    let root = root_span_for(&exporter, "payments-api");
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

/// The version a run measured is on its root span with where it came from.
#[test]
fn runner_emits_the_target_version_and_its_source_on_the_root_span() {
    use tumult_core::runner::{RunWindow, TargetVersionResolver, VersionSettle};

    struct Seen;
    impl TargetVersionResolver for Seen {
        fn resolve(&self, _window: &RunWindow) -> Result<Vec<VersionShare>, String> {
            Ok(vec![VersionShare {
                version: "1.5.1".into(),
                requests: 10,
            }])
        }

        fn settle(&self) -> VersionSettle {
            VersionSettle::ONCE
        }
    }

    let (provider, exporter, _lock) = setup_in_memory_provider();
    let target = |version: Option<&str>| ExperimentTarget {
        system: "checkout".into(),
        technology: None,
        environment: Some("demo".into()),
        criticality: None,
        namespace: None,
        version: version.map(str::to_string),
    };
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor);
    let controls = Arc::new(ControlRegistry::new());
    let config = RunConfig {
        version_resolver: Some(Arc::new(Seen)),
        ..RunConfig::default()
    };
    let version_on_root = |exp: &Experiment| -> (Option<String>, Option<String>) {
        exporter.reset();
        run_experiment(exp, &executor, &controls, &config).unwrap();
        let root = root_span_for(&exporter, "checkout");
        let attr = |key: &str| {
            root.attributes
                .iter()
                .find(|kv| kv.key.as_str() == key)
                .map(|kv| kv.value.as_str().into_owned())
        };
        (
            attr("resilience.target.version"),
            attr("resilience.target.version_source"),
        )
    };

    let mut declared = simple_experiment();
    declared.target = Some(target(Some("1.5.0")));
    assert_eq!(
        version_on_root(&declared),
        (Some("1.5.0".into()), Some("declared".into()))
    );

    let mut observed = simple_experiment();
    observed.target = Some(target(None));
    assert_eq!(
        version_on_root(&observed),
        (Some("1.5.1".into()), Some("observed".into()))
    );

    let _ = provider.shutdown();
}

fn text(span: &SpanData, key: &str) -> Option<String> {
    span.attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| kv.value.as_str().into_owned())
}

fn step_span(exporter: &InMemorySpanExporter, step: &str) -> SpanData {
    exporter
        .get_finished_spans()
        .unwrap_or_default()
        .into_iter()
        .find(|s| text(s, "resilience.fault.name").as_deref() == Some(step))
        .expect("a span that names the step")
}

/// The span of a step says what the step did, under which name, and which
/// plugin served it: the lake groups the durations of steps by these.
#[test]
fn the_span_of_a_step_says_what_it_did_and_which_plugin_served_it() {
    let (provider, exporter, _lock) = setup_in_memory_provider();

    let exp = Experiment {
        title: "step attributes".into(),
        method: vec![
            Activity {
                name: "cut-the-ledger-link".into(),
                activity_type: ActivityType::Action,
                provider: Provider::Native {
                    plugin: "tumult-network".into(),
                    function: "partition".into(),
                    arguments: std::collections::HashMap::new(),
                },
                ..Default::default()
            },
            Activity {
                name: "read-the-ledger-lag".into(),
                activity_type: ActivityType::Probe,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor);
    let controls = Arc::new(ControlRegistry::new());
    run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    let action = step_span(&exporter, "cut-the-ledger-link");
    assert_eq!(action.name, "resilience.action");
    assert_eq!(
        text(&action, "resilience.fault.type").as_deref(),
        Some("injection")
    );
    assert_eq!(
        text(&action, "resilience.plugin.name").as_deref(),
        Some("tumult-network")
    );
    // A command is served by the built-in executor, the plugin `process`.
    let probe = step_span(&exporter, "read-the-ledger-lag");
    assert_eq!(probe.name, "resilience.probe");
    assert_eq!(
        text(&probe, "resilience.fault.type").as_deref(),
        Some("observation")
    );
    assert_eq!(
        text(&probe, "resilience.plugin.name").as_deref(),
        Some("process")
    );

    let _ = provider.shutdown();
}
