use tumult_agentic::telemetry::{
    AgenticTraceContext, GenAiOperation, TelemetryEvidence, GEN_AI_OPERATION_NAME,
    GEN_AI_TOOL_NAME, RESILIENCE_AGENT_FAULT_TYPE, RESILIENCE_AGENT_SCENARIO,
    RESILIENCE_EXPERIMENT_ID, RESILIENCE_TRACE_ID,
};

#[test]
fn telemetry_evidence_uses_resilience_and_gen_ai_namespaces() {
    let evidence = TelemetryEvidence::new(
        "exp-agentic-001",
        "tool-timeout-fallback",
        "model_timeout",
        GenAiOperation::ExecuteTool,
    )
    .with_tool_name("search_docs")
    .with_trace_context(AgenticTraceContext::new(
        "4bf92f3577b34da6a3ce929d0e0e4736",
        "00f067aa0ba902b7",
    ));

    let attributes = evidence.span_attributes();

    assert_eq!(
        attributes.get(RESILIENCE_EXPERIMENT_ID),
        Some(&"exp-agentic-001".to_string())
    );
    assert_eq!(
        attributes.get(RESILIENCE_AGENT_SCENARIO),
        Some(&"tool-timeout-fallback".to_string())
    );
    assert_eq!(
        attributes.get(RESILIENCE_AGENT_FAULT_TYPE),
        Some(&"model_timeout".to_string())
    );
    assert_eq!(
        attributes.get(GEN_AI_OPERATION_NAME),
        Some(&"execute_tool".to_string())
    );
    assert_eq!(
        attributes.get(GEN_AI_TOOL_NAME),
        Some(&"search_docs".to_string())
    );
    assert_eq!(
        attributes.get(RESILIENCE_TRACE_ID),
        Some(&"4bf92f3577b34da6a3ce929d0e0e4736".to_string())
    );
}

#[test]
fn telemetry_evidence_redacts_payload_like_metadata_by_default() {
    let evidence = TelemetryEvidence::new(
        "exp-agentic-002",
        "malformed-json",
        "malformed_output",
        GenAiOperation::Chat,
    )
    .with_payload_metadata(1_024, 256, "sha256:abcd");

    let attributes = evidence.span_attributes();

    assert_eq!(attributes.get("gen_ai.prompt"), None);
    assert_eq!(attributes.get("gen_ai.completion"), None);
    assert_eq!(attributes.get("gen_ai.tool.payload"), None);
    assert_eq!(
        attributes.get("resilience.agent.payload.capture_policy"),
        Some(&"metadata_only".to_string())
    );
    assert_eq!(
        attributes.get("resilience.agent.input.bytes"),
        Some(&"1024".to_string())
    );
    assert_eq!(
        attributes.get("resilience.agent.output.bytes"),
        Some(&"256".to_string())
    );
}
