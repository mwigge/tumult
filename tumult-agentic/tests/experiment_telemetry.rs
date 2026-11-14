//! Proves the experiment side of two-sided observability: an *offline*
//! scenario-pack run (no live target, no collector) still emits the
//! `resilience.agentic.experiment` span tree via tumult-otel.

use tumult_agentic::smoke::run_scenario_pack_smoke;
use tumult_test_utils::capture_spans;

#[test]
fn offline_run_emits_experiment_span_with_fault_and_contract_events() {
    // Offline scenario-pack run — no live target, no collector.
    let (report, spans) = capture_spans(|| run_scenario_pack_smoke("malformed-json-recovery"));
    assert!(report.expect("runs offline").passed);

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
