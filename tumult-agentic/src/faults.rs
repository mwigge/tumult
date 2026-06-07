use serde::{Deserialize, Serialize};

use crate::model::AgenticError;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FaultSpec {
    ModelLatency {
        latency_ms: u64,
        probability: f64,
    },
    ModelTimeout {
        timeout_ms: u64,
        probability: f64,
    },
    ProviderError {
        code: u16,
        probability: f64,
    },
    RateLimit {
        retry_after_ms: u64,
        probability: f64,
    },
    MalformedOutput {
        probability: f64,
    },
    OutputTruncation {
        max_bytes: usize,
        probability: f64,
    },
    HallucinatedToolCall {
        tool_name: String,
        probability: f64,
    },
    ToolLatency {
        latency_ms: u64,
        probability: f64,
    },
    ToolFailure {
        error_type: String,
        probability: f64,
    },
    RetrievalPoisoning {
        document_count: u32,
        probability: f64,
    },
    ContextTruncation {
        max_tokens: u32,
        probability: f64,
    },
    TokenBudgetExhaustion {
        max_tokens: u32,
        probability: f64,
    },
    RetryLoopPressure {
        max_retries: u32,
        probability: f64,
    },
}

impl FaultSpec {
    #[must_use]
    pub fn fault_type(&self) -> &'static str {
        match self {
            Self::ModelLatency { .. } => "model_latency",
            Self::ModelTimeout { .. } => "model_timeout",
            Self::ProviderError { .. } => "provider_error",
            Self::RateLimit { .. } => "rate_limit",
            Self::MalformedOutput { .. } => "malformed_output",
            Self::OutputTruncation { .. } => "output_truncation",
            Self::HallucinatedToolCall { .. } => "hallucinated_tool_call",
            Self::ToolLatency { .. } => "tool_latency",
            Self::ToolFailure { .. } => "tool_failure",
            Self::RetrievalPoisoning { .. } => "retrieval_poisoning",
            Self::ContextTruncation { .. } => "context_truncation",
            Self::TokenBudgetExhaustion { .. } => "token_budget_exhaustion",
            Self::RetryLoopPressure { .. } => "retry_loop_pressure",
        }
    }

    #[must_use]
    pub fn probability(&self) -> f64 {
        match self {
            Self::ModelLatency { probability, .. }
            | Self::ModelTimeout { probability, .. }
            | Self::ProviderError { probability, .. }
            | Self::RateLimit { probability, .. }
            | Self::MalformedOutput { probability }
            | Self::OutputTruncation { probability, .. }
            | Self::HallucinatedToolCall { probability, .. }
            | Self::ToolLatency { probability, .. }
            | Self::ToolFailure { probability, .. }
            | Self::RetrievalPoisoning { probability, .. }
            | Self::ContextTruncation { probability, .. }
            | Self::TokenBudgetExhaustion { probability, .. }
            | Self::RetryLoopPressure { probability, .. } => *probability,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultTargetResponse {
    pub body: String,
    pub latency_ms: u64,
    pub retry_count: u32,
    pub tool_calls: u32,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub fallback_used: bool,
    pub tool_name: Option<String>,
    pub retrieved_documents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultOutcome {
    pub fault_type: String,
    pub label: String,
    pub response: FaultTargetResponse,
}

#[derive(Debug, Clone)]
pub struct FaultEngine {
    state: u64,
}

impl FaultEngine {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    #[must_use]
    pub fn should_apply(&mut self, fault: &FaultSpec) -> bool {
        self.next_unit_f64() < fault.probability()
    }

    fn next_unit_f64(&mut self) -> f64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let value = self.state >> 11;
        #[allow(clippy::cast_precision_loss)]
        // Converts deterministic PRNG state into a [0, 1) probability.
        {
            value as f64 / ((1_u64 << 53) as f64)
        }
    }
}

/// Apply a fault to a metadata-only target response.
///
/// # Errors
///
/// Returns [`AgenticError::InvalidConfig`] if a fault parameter is invalid.
pub fn apply_fault(
    fault: &FaultSpec,
    mut response: FaultTargetResponse,
) -> Result<FaultOutcome, AgenticError> {
    let label = format!("{}.injected", fault.fault_type());
    match fault {
        FaultSpec::ModelLatency { latency_ms, .. } | FaultSpec::ToolLatency { latency_ms, .. } => {
            response.latency_ms = response.latency_ms.saturating_add(*latency_ms);
        }
        FaultSpec::ModelTimeout { timeout_ms, .. } => {
            response.latency_ms = response.latency_ms.saturating_add(*timeout_ms);
            response.body = r#"{"error":"model_timeout"}"#.to_string();
        }
        FaultSpec::ProviderError { code, .. } => {
            response.body = format!(r#"{{"error":"provider_error","code":{code}}}"#);
        }
        FaultSpec::RateLimit { retry_after_ms, .. } => {
            response.retry_count = response.retry_count.saturating_add(1);
            response.latency_ms = response.latency_ms.saturating_add(*retry_after_ms);
        }
        FaultSpec::MalformedOutput { .. } => {
            response.body = "{malformed-json".to_string();
        }
        FaultSpec::OutputTruncation { max_bytes, .. } => {
            response.body.truncate(*max_bytes);
        }
        FaultSpec::HallucinatedToolCall { tool_name, .. } => {
            response.tool_name = Some(tool_name.clone());
            response.tool_calls = response.tool_calls.saturating_add(1);
        }
        FaultSpec::ToolFailure { error_type, .. } => {
            response.body = format!(r#"{{"tool_error":"{error_type}"}}"#);
        }
        FaultSpec::RetrievalPoisoning { document_count, .. } => {
            response
                .retrieved_documents
                .extend((0..*document_count).map(|idx| format!("poisoned-document-{idx}")));
        }
        FaultSpec::ContextTruncation { max_tokens, .. }
        | FaultSpec::TokenBudgetExhaustion { max_tokens, .. } => {
            let current = response.input_tokens.saturating_add(response.output_tokens);
            if current > *max_tokens {
                response.output_tokens = max_tokens.saturating_sub(response.input_tokens);
            }
        }
        FaultSpec::RetryLoopPressure { max_retries, .. } => {
            response.retry_count = *max_retries;
        }
    }

    Ok(FaultOutcome {
        fault_type: fault.fault_type().to_string(),
        label,
        response,
    })
}
