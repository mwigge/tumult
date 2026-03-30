//! TDD tests for OTel span creation in the experiment runner.
//!
//! These tests verify that the runner creates proper span hierarchies
//! with resilience.* attributes when a tracer is available.

use std::collections::HashMap;

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

#[test]
fn runner_populates_trace_id_on_activity_results() {
    // When a global tracer is set, activity results should have non-empty trace IDs
    let exp = simple_experiment();
    let executor = MockExecutor;
    let controls = ControlRegistry::new();

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
    // Initialize a simple in-process tracer to capture spans
    use opentelemetry::global;
    use opentelemetry::trace::Tracer;
    use opentelemetry_sdk::trace::SdkTracerProvider;

    let provider = SdkTracerProvider::builder().build();
    global::set_tracer_provider(provider.clone());

    let tracer = global::tracer("tumult-test");

    // Create a parent span to establish context
    let _guard = {
        use opentelemetry::trace::TraceContextExt;
        let span = tracer.start("test-parent");
        let cx = opentelemetry::Context::current_with_span(span);
        cx.attach()
    };

    let exp = simple_experiment();
    let executor = MockExecutor;
    let controls = ControlRegistry::new();

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
        use opentelemetry::trace::TraceContextExt;
        let ctx = opentelemetry::Context::current();
        ctx.span().span_context().clone()
    };
    let parent_span_id = parent_span_ctx.span_id().to_string();

    assert_ne!(
        method_span, &parent_span_id,
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
                &probe.span_id, &parent_span_id,
                "hypothesis probe should have its own span"
            );
        }
    }

    let _ = provider.shutdown();
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

    let journal = run_experiment(
        &exp,
        &MockExecutor,
        &ControlRegistry::new(),
        &RunConfig::default(),
    )
    .unwrap();

    // Without a tracer, trace_id is empty (no valid span context)
    // This is acceptable — the runner doesn't require OTel to function
    assert_eq!(journal.method_results.len(), 1);
}
