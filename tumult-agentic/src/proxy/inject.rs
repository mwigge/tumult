//! Fault classification and body mutation for proxied requests.

use std::time::Duration;

use crate::faults::{apply_fault, FaultSpec, FaultTargetResponse};

/// What a single applied fault does to a proxied request.
pub(crate) enum Injection {
    Delay(Duration),
    ShortCircuit {
        status: u16,
        body: String,
        retry_after_ms: Option<u64>,
    },
    MutateBody(FaultSpec),
    Internal,
}

pub(crate) fn classify(fault: &FaultSpec) -> Injection {
    match fault {
        FaultSpec::ModelLatency { latency_ms, .. } | FaultSpec::ToolLatency { latency_ms, .. } => {
            Injection::Delay(Duration::from_millis(*latency_ms))
        }
        FaultSpec::RateLimit { retry_after_ms, .. } => Injection::ShortCircuit {
            status: 429,
            body: error_body("rate_limit_error"),
            retry_after_ms: Some(*retry_after_ms),
        },
        FaultSpec::ProviderError { code, .. } => Injection::ShortCircuit {
            status: *code,
            body: error_body("provider_error"),
            retry_after_ms: None,
        },
        FaultSpec::ModelTimeout { .. } => Injection::ShortCircuit {
            status: 504,
            body: error_body("model_timeout"),
            retry_after_ms: None,
        },
        FaultSpec::MalformedOutput { .. }
        | FaultSpec::OutputTruncation { .. }
        | FaultSpec::ToolFailure { .. }
        | FaultSpec::RetrievalPoisoning { .. } => Injection::MutateBody(fault.clone()),
        FaultSpec::HallucinatedToolCall { .. }
        | FaultSpec::ContextTruncation { .. }
        | FaultSpec::TokenBudgetExhaustion { .. }
        | FaultSpec::RetryLoopPressure { .. } => Injection::Internal,
    }
}

pub(crate) fn error_body(kind: &str) -> String {
    format!(r#"{{"type":"error","error":{{"type":"{kind}","message":"injected by tumult"}}}}"#)
}

/// Apply a body-mutating fault by reusing the shared [`apply_fault`] mutator so
/// the proxy and the offline engine produce identical contamination.
pub(crate) fn mutate_body(fault: &FaultSpec, body: String) -> String {
    let response = FaultTargetResponse {
        body,
        latency_ms: 0,
        retry_count: 0,
        tool_calls: 0,
        input_tokens: 0,
        output_tokens: 0,
        fallback_used: false,
        tool_name: None,
        retrieved_documents: Vec::new(),
    };
    match apply_fault(fault, response) {
        Ok(outcome) => outcome.response.body,
        Err(_) => "{malformed-json".to_string(),
    }
}
