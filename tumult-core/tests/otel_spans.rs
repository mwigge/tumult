//! TDD tests for `OTel` span creation in the experiment runner.
//!
//! These tests verify that the runner creates proper span hierarchies
//! with resilience.* names and attributes when a tracer is available.
//!
//! The tests are split into cohesive submodules to keep each file small:
//!   * `trace_ids`   — trace/span id propagation across activities
//!   * `span_names`  — resilience.* span name emission

use std::collections::HashMap;
use std::sync::Mutex;

use opentelemetry::global;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

// Global mutex to serialize tests that modify the global tracer provider.
// The OTel global tracer provider is process-wide state; concurrent modification
// causes test flakiness. This mutex ensures only one such test runs at a time.
static TRACER_LOCK: Mutex<()> = Mutex::new(());

use tumult_core::runner::{ActivityExecutor, ActivityOutcome};
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

/// Helper: collect all span names from the in-memory exporter.
fn span_names(exporter: &InMemorySpanExporter) -> Vec<String> {
    exporter
        .get_finished_spans()
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.name.to_string())
        .collect()
}

#[path = "otel_spans/trace_ids.rs"]
mod trace_ids;

#[path = "otel_spans/span_names.rs"]
mod span_names_tests;
