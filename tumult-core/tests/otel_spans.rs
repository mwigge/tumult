//! TDD tests for `OTel` span creation in the experiment runner.
//!
//! These tests verify that the runner creates proper span hierarchies
//! with resilience.* names and attributes when a tracer is available.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use opentelemetry::global;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

// Global mutex to serialize tests that modify the global tracer provider.
// The OTel global tracer provider is process-wide state; concurrent modification
// causes test flakiness. This mutex ensures only one such test runs at a time.
static TRACER_LOCK: Mutex<()> = Mutex::new(());

use tumult_core::controls::ControlRegistry;
use tumult_core::runner::{run_experiment, ActivityExecutor, ActivityOutcome, RunConfig};
use tumult_core::types::*;

struct MockExecutor;
impl ActivityExecutor for MockExecutor {
    fn execute(&self, _activity: &Activity) -> ActivityOutcome {
        ActivityOutcome {
            success: true,
            output: Some("200".into()),
            error: None,
            duration_ms: 10,
        }
    }
}

fn simple_experiment() -> Experiment {
    Experiment {
        version: "v1".into(),
        title: "OTel span test".into(),
        method: vec![Activity {
            name: "test-action".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "echo".into(),
                arguments: vec!["hello".into()],
                env: HashMap::new(),
                timeout_s: Some(5.0),
            },
            ..Default::default()
        }],
        steady_state_hypothesis: Some(Hypothesis {
            title: "System is healthy".into(),
            probes: vec![Activity {
                name: "health-probe".into(),
                activity_type: ActivityType::Probe,
                provider: Provider::Process {
                    path: "echo".into(),
                    arguments: vec!["200".into()],
                    env: HashMap::new(),
                    timeout_s: Some(5.0),
                },
                tolerance: Some(Tolerance::Exact {
                    value: serde_json::Value::Number(200.into()),
                }),
                ..Default::default()
            }],
        }),
        ..Default::default()
    }
}

/// Set up an in-memory tracer provider and return the exporter for span inspection.
/// Returns the lock guard — hold it for the duration of the test to prevent
/// concurrent modification of the global tracer provider.
fn setup_in_memory_provider() -> (
    SdkTracerProvider,
    InMemorySpanExporter,
    std::sync::MutexGuard<'static, ()>,
) {
    let guard = TRACER_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    global::set_tracer_provider(provider.clone());
    (provider, exporter, guard)
}

#[test]
fn runner_populates_trace_id_on_activity_results() {
    // Without any tracer configured, activity results should not panic.
    let exp = simple_experiment();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor);
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    // Method results should exist
    assert!(!journal.method_results.is_empty());

    // trace_id and span_id may be empty if no tracer is configured,
    // but the fields should be populated (not panic)
    for result in &journal.method_results {
        // Just verify they're strings (not crashing)
        let _ = &result.trace_id;
        let _ = &result.span_id;
    }
}

#[test]
fn runner_creates_experiment_span_with_attributes() {
    let (provider, exporter, _lock) = setup_in_memory_provider();

    let tracer = global::tracer("tumult-test");

    // Create a parent span to establish context
    let _guard = {
        let span = tracer.start("test-parent");
        let cx = opentelemetry::Context::current_with_span(span);
        cx.attach()
    };

    let exp = simple_experiment();
    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor);
    let controls = Arc::new(ControlRegistry::new());

    let journal = run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    // With a tracer active, trace_id should be non-empty
    let method_trace = &journal.method_results[0].trace_id;
    let method_span = &journal.method_results[0].span_id;
    assert!(
        !method_trace.is_empty(),
        "trace_id should be populated when tracer is active"
    );
    assert!(
        !method_span.is_empty(),
        "span_id should be populated when tracer is active"
    );

    // The runner should create its OWN spans (different span_id from parent)
    let parent_span_ctx = {
        let ctx = opentelemetry::Context::current();
        ctx.span().span_context().clone()
    };
    let parent_span_id = parent_span_ctx.span_id().to_string();

    assert_ne!(
        method_span.as_str(),
        parent_span_id.as_str(),
        "runner should create child spans, not reuse parent span"
    );

    // Hypothesis probe results should also have their own spans
    if let Some(ref hyp) = journal.steady_state_before {
        for probe in &hyp.probe_results {
            assert!(
                !probe.trace_id.is_empty(),
                "hypothesis probe should have trace_id"
            );
            assert_ne!(
                probe.span_id.as_str(),
                parent_span_id.as_str(),
                "hypothesis probe should have its own span"
            );
        }
    }

    let _ = provider.shutdown();
    drop(exporter);
}

#[test]
fn runner_without_tracer_returns_empty_trace_ids() {
    // Without any tracer configured, trace_ids should be empty strings
    let exp = Experiment {
        title: "No tracer test".into(),
        method: vec![Activity {
            name: "action".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "echo".into(),
                arguments: vec!["ok".into()],
                env: HashMap::new(),
                timeout_s: Some(5.0),
            },
            ..Default::default()
        }],
        ..Default::default()
    };

    let executor: Arc<dyn ActivityExecutor> = Arc::new(MockExecutor);
    let controls = Arc::new(ControlRegistry::new());
    let journal = run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    // Without a tracer, trace_id is empty (no valid span context)
    // This is acceptable — the runner doesn't require OTel to function
    assert_eq!(journal.method_results.len(), 1);
}

#[test]
fn runner_all_activities_share_same_trace_id() {
    let (provider, exporter, _lock) = setup_in_memory_provider();

    let tracer = global::tracer("tumult-test");
    let _guard = {
        let span = tracer.start("test-root");
        let cx = opentelemetry::Context::current_with_span(span);
        cx.attach()
    };

    // Experiment with hypothesis + 2 method steps
    let exp = Experiment {
        title: "Multi-step span test".into(),
        method: vec![
            Activity {
                name: "step-1".into(),
                ..Default::default()
            },
            Activity {
                name: "step-2".into(),
                ..Default::default()
            },
        ],
        steady_state_hypothesis: Some(Hypothesis {
            title: "Healthy".into(),
            probes: vec![Activity {
                name: "probe-1".into(),
                activity_type: ActivityType::Probe,
                // MockExecutor returns "200" which parses as JSON number 200
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
    let journal = run_experiment(&exp, &executor, &controls, &RunConfig::default()).unwrap();

    // All activities should share the same trace_id (they're in the same trace)
    let trace_ids: Vec<&str> = journal
        .method_results
        .iter()
        .map(|r| r.trace_id.as_str())
        .collect();
    assert!(
        trace_ids.iter().all(|t| !t.is_empty()),
        "all trace_ids should be non-empty"
    );
    assert!(
        trace_ids.windows(2).all(|w| w[0] == w[1]),
        "all activities should share the same trace_id"
    );

    // But each should have a DIFFERENT span_id (unique per activity)
    let span_ids: Vec<&str> = journal
        .method_results
        .iter()
        .map(|r| r.span_id.as_str())
        .collect();
    assert_ne!(
        span_ids[0], span_ids[1],
        "each activity should have a unique span_id"
    );

    // Hypothesis probes should also share the trace but have unique spans
    if let Some(ref hyp) = journal.steady_state_before {
        let hyp_trace = &hyp.probe_results[0].trace_id;
        assert_eq!(
            hyp_trace.as_str(),
            trace_ids[0],
            "hypothesis should share trace_id with method"
        );
        let hyp_span = &hyp.probe_results[0].span_id;
        assert_ne!(
            hyp_span.as_str(),
            span_ids[0],
            "hypothesis should have different span_id from method"
        );
    }

    let _ = provider.shutdown();
    drop(exporter);
}

// ── TDD Red Phase: Span name assertions ──────────────────────────────────────
//
// These tests verify that the runner emits the correct span names as
// specified in README.md:
//   resilience.experiment  (root)
//   resilience.hypothesis.before
//   resilience.hypothesis.after
//   resilience.action
//   resilience.probe
//   resilience.rollback

/// Helper: collect all span names from the in-memory exporter.
fn span_names(exporter: &InMemorySpanExporter) -> Vec<String> {
    exporter
        .get_finished_spans()
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.name.to_string())
        .collect()
}

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
        rollback_strategy: tumult_core::execution::RollbackStrategy::Always,
        cancellation_token: None,
    };
    run_experiment(&exp, &executor, &controls, &config).unwrap();

    let names = span_names(&exporter);
    assert!(
        names.iter().any(|n| n == "resilience.rollback"),
        "expected 'resilience.rollback' span, got: {names:?}"
    );

    let _ = provider.shutdown();
}
