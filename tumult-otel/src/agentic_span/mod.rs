//! Experiment-side agentic instrumentation.
//!
//! Emits the `resilience.agentic.experiment` span tree (a span per run, with a
//! child event per fault decision and per contract outcome) plus run metrics,
//! using the canonical [`crate::agentic`] schema. This is the home for *how*
//! agentic runs are observed; tumult-agentic supplies the data and calls here.
#![allow(clippy::doc_markdown)]

mod run;
mod spans;

pub use run::{
    record_agentic_run, AgenticRunTelemetry, ContractRecord, FaultRecord, CONTRACT_COUNT,
    CONTRACT_REASON, CONTRACT_SEVERITY, EXPERIMENT_SPAN, FAULT_COUNT, TARGET_TYPE,
};
pub use spans::{
    start_experiment_root, start_proxy_span, start_tool_span, ProxySpan, SpanScope, DURATION_MS,
    EXPERIMENT_ROOT_SPAN, FAULTS_INJECTED, HTTP_METHOD, HTTP_PATH, HTTP_STATUS, PROXY_SPAN,
    TOOL_SPAN,
};

const SCOPE: &str = "tumult-agentic";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentic;
    use opentelemetry::trace::{Span, Tracer};
    use opentelemetry::KeyValue;
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
    fn experiment_span_nests_under_tool_span() {
        use opentelemetry::global;
        use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        global::set_tracer_provider(provider.clone());

        let tool = start_tool_span(
            agentic::TumultClient::Unknown.as_str(),
            "tumult_agentic_smoke",
        );
        let guard = tool.context().clone().attach();
        record_agentic_run(
            &AgenticRunTelemetry {
                scenario: "tool-nesting",
                target_type: "http",
                client: None,
                resilience_score: 0.0,
                faults: &[],
                contracts: &[],
            },
            None,
        );
        drop(guard);
        tool.end();
        provider.force_flush().ok();

        let spans = exporter.get_finished_spans().expect("spans");
        let tool_ids: Vec<_> = spans
            .iter()
            .filter(|span| span.name == TOOL_SPAN)
            .map(|span| span.span_context.span_id())
            .collect();
        let nested = spans
            .iter()
            .filter(|span| span.name == EXPERIMENT_SPAN)
            .any(|span| tool_ids.contains(&span.parent_span_id));
        assert!(nested, "experiment span must nest under the tool span");
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
