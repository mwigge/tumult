use tumult_agentic::faults::{
    apply_fault, FaultEngine, FaultOutcome, FaultSpec, FaultTargetResponse,
};

fn target_response() -> FaultTargetResponse {
    FaultTargetResponse {
        body: r#"{"ok":true,"tool":"lookup_order"}"#.to_string(),
        latency_ms: 25,
        retry_count: 0,
        tool_calls: 1,
        input_tokens: 10,
        output_tokens: 20,
        fallback_used: false,
        tool_name: Some("lookup_order".to_string()),
        retrieved_documents: vec!["primary order record".to_string()],
    }
}

#[test]
fn seeded_engine_makes_deterministic_fault_decisions() {
    let fault = FaultSpec::MalformedOutput { probability: 0.5 };
    let mut first = FaultEngine::new(42);
    let mut second = FaultEngine::new(42);

    let first_decisions = [
        first.should_apply(&fault),
        first.should_apply(&fault),
        first.should_apply(&fault),
        first.should_apply(&fault),
    ];
    let second_decisions = [
        second.should_apply(&fault),
        second.should_apply(&fault),
        second.should_apply(&fault),
        second.should_apply(&fault),
    ];

    assert_eq!(
        first_decisions, second_decisions,
        "expected identical seeds to produce identical fault application decisions"
    );
}

#[test]
fn malformed_output_fault_records_low_cardinality_label() {
    let outcome = apply_fault(
        &FaultSpec::MalformedOutput { probability: 1.0 },
        target_response(),
    )
    .expect("expected malformed output fault to apply cleanly");

    assert_eq!(outcome.fault_type, "malformed_output");
    assert_eq!(outcome.label, "malformed_output.injected");
    assert_eq!(outcome.response.body, "{malformed-json");
}

#[test]
fn all_fault_types_have_stable_labels() {
    let faults = vec![
        FaultSpec::ModelLatency {
            latency_ms: 100,
            probability: 1.0,
        },
        FaultSpec::ModelTimeout {
            timeout_ms: 50,
            probability: 1.0,
        },
        FaultSpec::ProviderError {
            code: 500,
            probability: 1.0,
        },
        FaultSpec::RateLimit {
            retry_after_ms: 250,
            probability: 1.0,
        },
        FaultSpec::MalformedOutput { probability: 1.0 },
        FaultSpec::OutputTruncation {
            max_bytes: 8,
            probability: 1.0,
        },
        FaultSpec::HallucinatedToolCall {
            tool_name: "ghost_tool".to_string(),
            probability: 1.0,
        },
        FaultSpec::ToolLatency {
            latency_ms: 90,
            probability: 1.0,
        },
        FaultSpec::ToolFailure {
            error_type: "unavailable".to_string(),
            probability: 1.0,
        },
        FaultSpec::RetrievalPoisoning {
            document_count: 2,
            probability: 1.0,
        },
        FaultSpec::ContextTruncation {
            max_tokens: 12,
            probability: 1.0,
        },
        FaultSpec::TokenBudgetExhaustion {
            max_tokens: 15,
            probability: 1.0,
        },
        FaultSpec::RetryLoopPressure {
            max_retries: 3,
            probability: 1.0,
        },
    ];

    let labels = faults
        .into_iter()
        .map(|fault| {
            apply_fault(&fault, target_response())
                .map(|outcome| outcome.label)
                .unwrap_or_else(|error| error.to_string())
        })
        .collect::<Vec<_>>();

    assert_eq!(
        labels,
        vec![
            "model_latency.injected",
            "model_timeout.injected",
            "provider_error.injected",
            "rate_limit.injected",
            "malformed_output.injected",
            "output_truncation.injected",
            "hallucinated_tool_call.injected",
            "tool_latency.injected",
            "tool_failure.injected",
            "retrieval_poisoning.injected",
            "context_truncation.injected",
            "token_budget_exhaustion.injected",
            "retry_loop_pressure.injected",
        ]
    );
}

#[test]
fn fault_outcome_omits_raw_context_by_default() {
    let outcome = apply_fault(
        &FaultSpec::RetrievalPoisoning {
            document_count: 2,
            probability: 1.0,
        },
        target_response(),
    )
    .expect("expected retrieval poisoning fault to apply cleanly");

    assert_eq!(
        outcome,
        FaultOutcome {
            fault_type: "retrieval_poisoning".to_string(),
            label: "retrieval_poisoning.injected".to_string(),
            response: outcome.response.clone(),
        }
    );
    assert!(
        !outcome.label.contains("primary order record"),
        "expected fault labels to stay metadata-only and omit retrieved document content"
    );
}
