use serde::{Deserialize, Serialize};

use crate::adapters::AgentResponse;
use crate::model::ContractOutcome;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContractSpec {
    ValidJson {
        severity: Option<f64>,
    },
    MaxLatency {
        max_ms: u64,
        severity: Option<f64>,
    },
    RetryBudget {
        max_retries: u32,
        severity: Option<f64>,
    },
    MaxToolCalls {
        max_calls: u32,
        severity: Option<f64>,
    },
    MaxTokenUsage {
        max_tokens: u32,
        severity: Option<f64>,
    },
    FallbackUsed {
        severity: Option<f64>,
    },
    GracefulError {
        severity: Option<f64>,
    },
    RequiredCitation {
        severity: Option<f64>,
    },
    NoPii {
        severity: Option<f64>,
    },
    NoSecretLeakage {
        severity: Option<f64>,
    },
}

impl ContractSpec {
    #[must_use]
    pub fn contract_type(&self) -> &'static str {
        match self {
            Self::ValidJson { .. } => "valid_json",
            Self::MaxLatency { .. } => "max_latency",
            Self::RetryBudget { .. } => "retry_budget",
            Self::MaxToolCalls { .. } => "max_tool_calls",
            Self::MaxTokenUsage { .. } => "max_token_usage",
            Self::FallbackUsed { .. } => "fallback_used",
            Self::GracefulError { .. } => "graceful_error",
            Self::RequiredCitation { .. } => "required_citation",
            Self::NoPii { .. } => "no_pii",
            Self::NoSecretLeakage { .. } => "no_secret_leakage",
        }
    }
}

#[must_use]
pub fn evaluate_contract(
    scenario: &str,
    contract: &ContractSpec,
    response: &AgentResponse,
) -> ContractOutcome {
    let severity = severity(contract);
    let (passed, reason) = match contract {
        ContractSpec::ValidJson { .. } => {
            match serde_json::from_str::<serde_json::Value>(&response.body) {
                Ok(_) => (true, None),
                Err(_) => (false, Some("invalid_json".to_string())),
            }
        }
        ContractSpec::MaxLatency { max_ms, .. } => (
            response.latency_ms <= *max_ms,
            failure_reason(response.latency_ms <= *max_ms, "latency_exceeded"),
        ),
        ContractSpec::RetryBudget { max_retries, .. } => (
            response.retry_count <= *max_retries,
            failure_reason(
                response.retry_count <= *max_retries,
                "retry_budget_exceeded",
            ),
        ),
        ContractSpec::MaxToolCalls { max_calls, .. } => (
            response.tool_calls <= *max_calls,
            failure_reason(
                response.tool_calls <= *max_calls,
                "tool_call_budget_exceeded",
            ),
        ),
        ContractSpec::MaxTokenUsage { max_tokens, .. } => {
            let total = response.input_tokens.saturating_add(response.output_tokens);
            (
                total <= *max_tokens,
                failure_reason(total <= *max_tokens, "token_budget_exceeded"),
            )
        }
        ContractSpec::FallbackUsed { .. } => (
            response.fallback_used,
            failure_reason(response.fallback_used, "fallback_not_used"),
        ),
        ContractSpec::GracefulError { .. } => (
            !response.body.to_lowercase().contains("panic"),
            failure_reason(
                !response.body.to_lowercase().contains("panic"),
                "ungraceful_error",
            ),
        ),
        ContractSpec::RequiredCitation { .. } => (
            response.body.contains("http") || response.body.contains('['),
            failure_reason(
                response.body.contains("http") || response.body.contains('['),
                "citation_missing",
            ),
        ),
        ContractSpec::NoPii { .. } => (
            !response.body.contains('@'),
            failure_reason(!response.body.contains('@'), "pii_detected:email"),
        ),
        ContractSpec::NoSecretLeakage { .. } => (
            !response.body.contains("sk-") && !response.body.contains("SECRET"),
            failure_reason(
                !response.body.contains("sk-") && !response.body.contains("SECRET"),
                "secret_detected:api_key",
            ),
        ),
    };

    ContractOutcome {
        contract_type: contract.contract_type().to_string(),
        scenario: scenario.to_string(),
        passed,
        reason,
        severity,
    }
}

fn severity(contract: &ContractSpec) -> f64 {
    match contract {
        ContractSpec::ValidJson { severity }
        | ContractSpec::MaxLatency { severity, .. }
        | ContractSpec::RetryBudget { severity, .. }
        | ContractSpec::MaxToolCalls { severity, .. }
        | ContractSpec::MaxTokenUsage { severity, .. }
        | ContractSpec::FallbackUsed { severity }
        | ContractSpec::GracefulError { severity }
        | ContractSpec::RequiredCitation { severity }
        | ContractSpec::NoPii { severity }
        | ContractSpec::NoSecretLeakage { severity } => severity.unwrap_or(1.0),
    }
}

fn failure_reason(passed: bool, reason: &str) -> Option<String> {
    if passed {
        None
    } else {
        Some(reason.to_string())
    }
}
