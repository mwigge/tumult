//! Proves the experiment side of two-sided observability: an *offline*
//! scenario-pack run (no live target, no collector) still emits the
//! `resilience.agentic.experiment` span tree via tumult-otel.

use opentelemetry::global;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};
use tumult_agentic::smoke::run_scenario_pack_smoke;

#[test]
fn offline_run_emits_experiment_span_with_fault_and_contract_events() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    global::set_tracer_provider(provider.clone());

    // Offline scenario-pack run — no live target, no collector.
    let report = run_scenario_pack_smoke("malformed-json-recovery").expect("runs offline");
    assert!(report.passed);

    provider.force_flush().ok();
    let spans = exporter.get_finished_spans().expect("captured spans");

    // At least one experiment span carries both a fault and a contract event.
    let has_experiment_span = spans
        .iter()
        .filter(|span| span.name == "resilience.agentic.experiment")
        .any(|span| {
            let events: Vec<&str> = span.events.iter().map(|e| e.name.as_ref()).collect();
            events.contains(&"fault") && events.contains(&"contract")
        });

    assert!(
        has_experiment_span,
        "offline run must emit a resilience.agentic.experiment span with fault + contract events"
    );
}
