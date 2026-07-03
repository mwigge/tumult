//! Experiment-run recording: the `resilience.agentic.experiment` span tree and
//! the accompanying run metrics for a completed agentic run.

use opentelemetry::trace::{Span, SpanKind, Status, Tracer};
use opentelemetry::{global, Context, KeyValue};

use super::SCOPE;
use crate::agentic;

pub const EXPERIMENT_SPAN: &str = "resilience.agentic.experiment";
pub const TARGET_TYPE: &str = "resilience.agent.target_type";
pub const FAULT_COUNT: &str = "resilience.agent.fault.count";
pub const CONTRACT_COUNT: &str = "resilience.agent.contract.count";
pub const CONTRACT_REASON: &str = "resilience.agent.contract.reason";
pub const CONTRACT_SEVERITY: &str = "resilience.agent.contract.severity";

/// One fault decision in a run.
#[derive(Debug, Clone)]
pub struct FaultRecord {
    pub fault_type: String,
    pub applied: bool,
}

/// One contract outcome in a run.
#[derive(Debug, Clone)]
pub struct ContractRecord {
    pub contract_type: String,
    pub passed: bool,
    pub reason: Option<String>,
    pub severity: f64,
}

/// All data needed to instrument a completed agentic run from tumult's side.
#[derive(Debug, Clone)]
pub struct AgenticRunTelemetry<'a> {
    pub scenario: &'a str,
    pub target_type: &'a str,
    pub client: Option<&'a str>,
    pub resilience_score: f64,
    pub faults: &'a [FaultRecord],
    pub contracts: &'a [ContractRecord],
}

/// Emit the experiment span tree and run metrics for a completed agentic run.
///
/// `parent` optionally nests the experiment span under an inbound trace context
/// (e.g. an orchestrator's root span). When no OpenTelemetry provider is
/// installed (offline runs without a collector), the global tracer/meter are
/// no-ops and this is a cheap call that does not panic.
pub fn record_agentic_run(run: &AgenticRunTelemetry, parent: Option<&Context>) {
    let tracer = global::tracer(SCOPE);
    let client = run
        .client
        .unwrap_or_else(|| agentic::TumultClient::Unknown.as_str());

    let builder = tracer
        .span_builder(EXPERIMENT_SPAN)
        .with_kind(SpanKind::Internal)
        .with_attributes([
            KeyValue::new(agentic::RESILIENCE_AGENT_SCENARIO, run.scenario.to_string()),
            KeyValue::new(TARGET_TYPE, run.target_type.to_string()),
            KeyValue::new(agentic::TUMULT_CLIENT, client.to_string()),
            KeyValue::new(agentic::RESILIENCE_AGENT_SCORE, run.resilience_score),
            KeyValue::new(agentic::RESILIENCE_AGENT_CAPTURE_POLICY, "metadata_only"),
            KeyValue::new(FAULT_COUNT, count_i64(run.faults.len())),
            KeyValue::new(CONTRACT_COUNT, count_i64(run.contracts.len())),
        ]);

    let mut span = match parent {
        Some(context) => builder.start_with_context(&tracer, context),
        None => builder.start(&tracer),
    };

    for fault in run.faults {
        span.add_event(
            "fault",
            vec![
                KeyValue::new(
                    agentic::RESILIENCE_AGENT_FAULT_TYPE,
                    fault.fault_type.clone(),
                ),
                KeyValue::new(agentic::RESILIENCE_AGENT_FAULT_APPLIED, fault.applied),
            ],
        );
    }

    let mut any_failed = false;
    for contract in run.contracts {
        if !contract.passed {
            any_failed = true;
        }
        let mut attrs = vec![
            KeyValue::new(
                agentic::RESILIENCE_AGENT_CONTRACT,
                contract.contract_type.clone(),
            ),
            KeyValue::new(agentic::RESILIENCE_AGENT_CONTRACT_PASSED, contract.passed),
            KeyValue::new(CONTRACT_SEVERITY, contract.severity),
        ];
        if let Some(reason) = &contract.reason {
            attrs.push(KeyValue::new(CONTRACT_REASON, reason.clone()));
        }
        span.add_event("contract", attrs);
    }

    span.set_status(if any_failed {
        Status::error("one or more contracts failed")
    } else {
        Status::Ok
    });
    span.end();

    record_metrics(run, client, any_failed);
}

fn record_metrics(run: &AgenticRunTelemetry, client: &str, any_failed: bool) {
    let meter = global::meter(SCOPE);
    let attrs = [
        KeyValue::new(agentic::RESILIENCE_AGENT_SCENARIO, run.scenario.to_string()),
        KeyValue::new(agentic::TUMULT_CLIENT, client.to_string()),
    ];

    let applied = run.faults.iter().filter(|fault| fault.applied).count();
    let passed = run.contracts.iter().filter(|c| c.passed).count();
    let failed = run.contracts.len() - passed;

    meter
        .u64_counter("resilience.agent.faults_applied")
        .build()
        .add(applied as u64, &attrs);
    meter
        .u64_counter("resilience.agent.contracts_passed")
        .build()
        .add(passed as u64, &attrs);
    meter
        .u64_counter("resilience.agent.contracts_failed")
        .build()
        .add(failed as u64, &attrs);
    meter
        .f64_histogram("resilience.agent.score")
        .build()
        .record(run.resilience_score, &attrs);

    if any_failed {
        // Mirror the experiment-failed signal as a counter for alerting.
        meter
            .u64_counter("resilience.agent.runs_with_failure")
            .build()
            .add(1, &attrs);
    }
}

fn count_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
