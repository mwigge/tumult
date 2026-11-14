//! `record_agentic_run` in a process that never installed an OpenTelemetry
//! provider: an offline run without a collector.
//!
//! Separate integration-test binary with a single test: the global tracer
//! and meter providers are shared by every test of a binary, and this one
//! needs them never to have been set.

use opentelemetry::trace::TraceContextExt;
use opentelemetry::Context;
use tracing::Level;
use tumult_otel::agentic_span::{
    record_agentic_run, AgenticRunTelemetry, ContractRecord, FaultRecord,
};
use tumult_otel::logging::testing::LogCapture;

#[test]
fn a_run_recorded_without_a_provider_leaves_no_span_and_logs_no_problem() {
    let logs = LogCapture::start(Level::WARN);
    let faults = [FaultRecord {
        fault_type: "latency".into(),
        applied: true,
    }];
    let contracts = [ContractRecord {
        contract_type: "must-retry".into(),
        passed: false,
        reason: Some("gave up after one attempt".into()),
        severity: 0.8,
    }];

    record_agentic_run(
        &AgenticRunTelemetry {
            scenario: "offline",
            target_type: "http",
            client: None,
            resilience_score: 0.2,
            faults: &faults,
            contracts: &contracts,
        },
        None,
    );

    let logged = logs.snapshot();
    assert!(
        logged.events.is_empty(),
        "an offline run is not a problem to report: {}",
        logged.raw
    );
    assert!(
        !Context::current().span().span_context().is_valid(),
        "no span may stay active after the call"
    );
}
