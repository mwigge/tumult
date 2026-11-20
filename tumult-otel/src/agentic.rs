//! Canonical agentic telemetry schema.
//!
//! This is the single home for the agentic observability vocabulary —
//! `resilience.agent.*` attributes, the GenAI semantic-convention keys
//! (`gen_ai.*`) and the `tumult.client` identifier.
//! tumult-agentic consumes these rather than defining its own,
//! so the schema lives in exactly one place.
#![allow(clippy::doc_markdown)] // doc names standards (OpenTelemetry, GenAI)

use serde::{Deserialize, Serialize};

// ── resilience.* (agentic) ─────────────────────────────────────────────
/// Span attribute key for the agent scenario name.
pub const RESILIENCE_AGENT_SCENARIO: &str = "resilience.agent.scenario";
/// Span attribute key for the type of fault applied to the agent.
pub const RESILIENCE_AGENT_FAULT_TYPE: &str = "resilience.agent.fault.type";
/// Span attribute key recording whether the fault was actually applied.
pub const RESILIENCE_AGENT_FAULT_APPLIED: &str = "resilience.agent.fault.applied";
/// Span attribute key for the contract type evaluated.
pub const RESILIENCE_AGENT_CONTRACT: &str = "resilience.agent.contract";
/// Span attribute key recording whether the contract passed.
pub const RESILIENCE_AGENT_CONTRACT_PASSED: &str = "resilience.agent.contract.passed";
/// Span attribute key for the resilience score.
pub const RESILIENCE_AGENT_SCORE: &str = "resilience.agent.score";
/// Span attribute key for the payload capture policy (e.g. `metadata_only`).
pub const RESILIENCE_AGENT_CAPTURE_POLICY: &str = "resilience.agent.payload.capture_policy";

// ── gen_ai.* (OpenTelemetry GenAI semantic conventions) ────────────────
/// Span attribute key for the GenAI operation name.
pub const GEN_AI_OPERATION_NAME: &str = "gen_ai.operation.name";
/// Span attribute key for the invoked tool name.
pub const GEN_AI_TOOL_NAME: &str = "gen_ai.tool.name";
/// Span attribute key for the evaluation result.
pub const GEN_AI_EVALUATION_RESULT: &str = "gen_ai.evaluation.result";
/// Span attribute key for the model requested by the client.
pub const GEN_AI_REQUEST_MODEL: &str = "gen_ai.request.model";
/// Span attribute key for the model that produced the response.
pub const GEN_AI_RESPONSE_MODEL: &str = "gen_ai.response.model";
/// Span attribute key for the response finish reasons.
pub const GEN_AI_RESPONSE_FINISH_REASONS: &str = "gen_ai.response.finish_reasons";
/// Span attribute key for the GenAI provider system.
pub const GEN_AI_SYSTEM: &str = "gen_ai.system";
/// Span attribute key for the input token usage.
pub const GEN_AI_USAGE_INPUT_TOKENS: &str = "gen_ai.usage.input_tokens";
/// Span attribute key for the output token usage.
pub const GEN_AI_USAGE_OUTPUT_TOKENS: &str = "gen_ai.usage.output_tokens";

/// Resource attribute identifying which client produced (or is the target of)
/// a piece of agentic telemetry. The unifying tag across all four clients.
pub const TUMULT_CLIENT: &str = "tumult.client";

/// The agentic client a run targets, used as the `tumult.client` value so
/// telemetry from any client normalizes onto one schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TumultClient {
    ClaudeCode,
    Codex,
    Copilot,
    OpenCode,
    /// Fallback used when the client cannot be identified.
    Unknown,
}

impl TumultClient {
    /// The kebab-case `tumult.client` attribute value for this client.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Copilot => "copilot",
            Self::OpenCode => "opencode",
            Self::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tumult_client_attribute_values_are_kebab_case() {
        assert_eq!(TumultClient::ClaudeCode.as_str(), "claude-code");
        assert_eq!(TumultClient::Codex.as_str(), "codex");
        assert_eq!(TumultClient::Copilot.as_str(), "copilot");
        assert_eq!(TumultClient::OpenCode.as_str(), "opencode");
        assert_eq!(TumultClient::Unknown.as_str(), "unknown");
    }
}
