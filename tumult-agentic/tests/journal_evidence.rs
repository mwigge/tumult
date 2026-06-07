use tumult_agentic::journal::{
    encode_metadata_journal, AgenticJournalContract, AgenticJournalEvidence, AgenticJournalFault,
    AgenticJournalScenario, AgenticJournalToolCall, JournalTraceCorrelation,
};

#[test]
fn encoded_journal_contains_trace_correlation_without_raw_payloads() {
    let evidence = AgenticJournalEvidence {
        experiment_id: "exp-agentic-003".to_string(),
        run_id: "run-003".to_string(),
        capture_policy: "metadata_only".to_string(),
        trace: JournalTraceCorrelation {
            trace_id: "4bf92f3577b34da6a3ce929d0e0e4736".to_string(),
            span_id: "00f067aa0ba902b7".to_string(),
            parent_span_id: Some("7b34da6a3ce929d0".to_string()),
        },
        scenarios: vec![AgenticJournalScenario {
            name: "tool-timeout-fallback".to_string(),
            input_sha256: "sha256:input".to_string(),
            expected_behavior_sha256: Some("sha256:expected".to_string()),
        }],
        faults: vec![AgenticJournalFault {
            fault_type: "tool_timeout".to_string(),
            scenario: "tool-timeout-fallback".to_string(),
            applied: true,
            latency_ms: Some(750),
        }],
        contracts: vec![AgenticJournalContract {
            contract_type: "fallback_used".to_string(),
            scenario: "tool-timeout-fallback".to_string(),
            passed: false,
            reason: Some("fallback_not_used".to_string()),
            severity: 1.0,
        }],
        tool_calls: vec![AgenticJournalToolCall {
            tool_name: "search_docs".to_string(),
            operation: "execute_tool".to_string(),
            payload_sha256: "sha256:tool".to_string(),
            status: "timeout".to_string(),
        }],
        contract_pass_rate: 0.5,
        resilience_score: 62.5,
    };

    let encoded = encode_metadata_journal(&evidence).expect("journal encodes");

    assert!(encoded.contains("trace_id"));
    assert!(encoded.contains("span_id"));
    assert!(encoded.contains("metadata_only"));
    assert!(encoded.contains("payload_sha256"));
    assert!(!encoded.contains("user prompt with secret"));
    assert!(!encoded.contains("model completion text"));
    assert!(!encoded.contains("raw tool payload"));
}
