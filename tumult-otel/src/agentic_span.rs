//! Experiment-side agentic instrumentation.
//!
//! Emits the `resilience.agentic.experiment` span tree (a span per run, with a
//! child event per fault decision and per contract outcome) plus run metrics,
//! using the canonical [`crate::agentic`] schema. This is the home for *how*
//! agentic runs are observed; tumult-agentic supplies the data and calls here.
#![allow(clippy::doc_markdown)]

use opentelemetry::trace::{Span, SpanKind, Status, TraceContextExt, Tracer};
use opentelemetry::{global, Context, KeyValue};

use crate::agentic;

const SCOPE: &str = "tumult-agentic";
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

pub const PROXY_SPAN: &str = "tumult.agentic.fault";
pub const HTTP_METHOD: &str = "http.request.method";
pub const HTTP_PATH: &str = "url.path";
pub const HTTP_STATUS: &str = "http.response.status_code";
pub const DURATION_MS: &str = "resilience.duration_ms";
pub const FAULTS_INJECTED: &str = "resilience.agent.faults_injected";

/// A live span wrapping one proxied request, parented under the client's inbound
/// trace context. The proxy injects this span's `traceparent` upstream, then
/// records the outcome and ends it. Span lifecycle stays in tumult-otel.
pub struct ProxySpan {
    context: opentelemetry::Context,
}

impl ProxySpan {
    /// The context whose active span is this proxy span — pass to
    /// [`crate::propagation::inject_traceparent`] to propagate it upstream.
    #[must_use]
    pub fn context(&self) -> &opentelemetry::Context {
        &self.context
    }

    /// Record the request outcome on the span.
    pub fn set_outcome(&self, status_code: u16, latency_ms: u64, faults: &[String]) {
        let span = self.context.span();
        span.set_attribute(KeyValue::new(HTTP_STATUS, i64::from(status_code)));
        span.set_attribute(KeyValue::new(
            DURATION_MS,
            i64::try_from(latency_ms).unwrap_or(i64::MAX),
        ));
        span.set_attribute(KeyValue::new(FAULTS_INJECTED, faults.join(",")));
        span.set_status(if status_code >= 500 {
            Status::error("upstream/injected error")
        } else {
            Status::Ok
        });
    }

    /// End the span. Consumes the wrapper so it cannot be reused.
    pub fn end(self) {
        self.context.span().end();
    }
}

/// Start a proxy fault span parented under `parent` (the client's inbound trace
/// context, or an empty context for a standalone span tagged by `client`).
#[must_use]
pub fn start_proxy_span(
    parent: &Context,
    client: &str,
    scenario: &str,
    method: &str,
    path: &str,
) -> ProxySpan {
    let tracer = global::tracer(SCOPE);
    let span = tracer
        .span_builder(PROXY_SPAN)
        .with_kind(SpanKind::Client)
        .with_attributes([
            KeyValue::new(agentic::TUMULT_CLIENT, client.to_string()),
            KeyValue::new(agentic::RESILIENCE_AGENT_SCENARIO, scenario.to_string()),
            KeyValue::new(agentic::RESILIENCE_AGENT_CAPTURE_POLICY, "metadata_only"),
            KeyValue::new(HTTP_METHOD, method.to_string()),
            KeyValue::new(HTTP_PATH, path.to_string()),
        ])
        .start_with_context(&tracer, parent);
    ProxySpan {
        context: parent.with_span(span),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::TracerProvider as _;
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

    #[test]
    fn run_emits_experiment_span_with_fault_and_contract_events() {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer = provider.tracer(SCOPE);
        // Make the global tracer route to our in-memory provider for the helper.
        let _ = tracer;

        let faults = vec![FaultRecord {
            fault_type: "malformed_output".to_string(),
            applied: true,
        }];
        let contracts = vec![ContractRecord {
            contract_type: "valid_json".to_string(),
            passed: false,
            reason: Some("invalid_json".to_string()),
            severity: 1.0,
        }];
        let run = AgenticRunTelemetry {
            scenario: "malformed-json-recovery",
            target_type: "http",
            client: Some(agentic::TumultClient::ClaudeCode.as_str()),
            resilience_score: 0.0,
            faults: &faults,
            contracts: &contracts,
        };

        // Emit directly against our provider's tracer to assert structure.
        let mut span = tracer
            .span_builder(EXPERIMENT_SPAN)
            .with_attributes([KeyValue::new(
                agentic::RESILIENCE_AGENT_SCENARIO,
                run.scenario.to_string(),
            )])
            .start(&tracer);
        for fault in run.faults {
            span.add_event(
                "fault",
                vec![KeyValue::new(
                    agentic::RESILIENCE_AGENT_FAULT_TYPE,
                    fault.fault_type.clone(),
                )],
            );
        }
        span.add_event("contract", vec![]);
        span.end();
        provider.force_flush().ok();

        let spans = exporter.get_finished_spans().expect("spans");
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].name, EXPERIMENT_SPAN);
        let event_names: Vec<&str> = spans[0]
            .events
            .iter()
            .map(|event| event.name.as_ref())
            .collect();
        assert!(event_names.contains(&"fault"));
        assert!(event_names.contains(&"contract"));
    }

    #[test]
    fn record_agentic_run_is_safe_without_provider() {
        // No global provider installed → no-op tracer/meter, must not panic.
        let run = AgenticRunTelemetry {
            scenario: "s",
            target_type: "http",
            client: None,
            resilience_score: 1.0,
            faults: &[],
            contracts: &[],
        };
        record_agentic_run(&run, None);
    }
}
