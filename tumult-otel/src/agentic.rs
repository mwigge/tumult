//! Canonical agentic telemetry schema.
//!
//! This is the single home for the agentic observability vocabulary —
//! `resilience.agent.*` attributes, the GenAI semantic-convention keys
//! (`gen_ai.*`), the `tumult.client` identifier, and the
//! [`TelemetryEvidence`][crate::agentic::TelemetryEvidence] evidence record.
//! tumult-agentic consumes these rather than defining its own,
//! so the schema lives in exactly one place.
#![allow(clippy::doc_markdown)] // doc names standards (OpenTelemetry, GenAI)

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// ── resilience.* (agentic) ─────────────────────────────────────────────
/// Span attribute key for the experiment identifier.
pub const RESILIENCE_EXPERIMENT_ID: &str = "resilience.experiment.id";
/// Span attribute key for the run identifier.
pub const RESILIENCE_RUN_ID: &str = "resilience.run.id";
/// Span attribute key for the trace identifier.
pub const RESILIENCE_TRACE_ID: &str = "resilience.trace.id";
/// Span attribute key for the span identifier.
pub const RESILIENCE_SPAN_ID: &str = "resilience.span.id";
/// Span attribute key for the parent span identifier.
pub const RESILIENCE_PARENT_SPAN_ID: &str = "resilience.parent_span.id";
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
/// Span attribute key for the input payload size, in bytes.
pub const RESILIENCE_AGENT_INPUT_BYTES: &str = "resilience.agent.input.bytes";
/// Span attribute key for the output payload size, in bytes.
pub const RESILIENCE_AGENT_OUTPUT_BYTES: &str = "resilience.agent.output.bytes";
/// Span attribute key for the SHA-256 digest of the payload.
pub const RESILIENCE_AGENT_PAYLOAD_SHA256: &str = "resilience.agent.payload.sha256";

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

/// The GenAI operation a piece of agentic telemetry describes, used as the
/// `gen_ai.operation.name` value per the OpenTelemetry GenAI semantic
/// conventions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenAiOperation {
    /// A chat-completion call (message list in, message out).
    Chat,
    /// Content generation outside a chat context.
    GenerateContent,
    /// Invocation of a single tool or function.
    ExecuteTool,
    /// An embeddings request.
    Embeddings,
    /// Invocation of a (sub-)agent.
    InvokeAgent,
    /// Invocation of a multi-step workflow.
    InvokeWorkflow,
    /// Evaluation of a model or agent output.
    Evaluate,
}

impl GenAiOperation {
    /// The snake_case `gen_ai.operation.name` attribute value for this operation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::GenerateContent => "generate_content",
            Self::ExecuteTool => "execute_tool",
            Self::Embeddings => "embeddings",
            Self::InvokeAgent => "invoke_agent",
            Self::InvokeWorkflow => "invoke_workflow",
            Self::Evaluate => "evaluate",
        }
    }
}

/// Trace coordinates linking an agentic evidence record to the `OTel` span tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgenticTraceContext {
    /// The trace identifier.
    pub trace_id: String,
    /// The span identifier.
    pub span_id: String,
    /// The parent span identifier, when this span has a recorded parent.
    pub parent_span_id: Option<String>,
}

impl AgenticTraceContext {
    /// Create a context for a span with no recorded parent.
    #[must_use]
    pub fn new(trace_id: impl Into<String>, span_id: impl Into<String>) -> Self {
        Self {
            trace_id: trace_id.into(),
            span_id: span_id.into(),
            parent_span_id: None,
        }
    }

    /// Set the parent span identifier.
    #[must_use]
    pub fn with_parent_span_id(mut self, parent_span_id: impl Into<String>) -> Self {
        self.parent_span_id = Some(parent_span_id.into());
        self
    }
}

/// Size and hash metadata for a captured payload. Never carries the payload
/// body itself — the capture policy is metadata-only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadMetadata {
    /// Size of the input payload, in bytes.
    pub input_bytes: u64,
    /// Size of the output payload, in bytes.
    pub output_bytes: u64,
    /// SHA-256 hex digest of the payload.
    pub payload_sha256: String,
}

/// One piece of agentic telemetry evidence: what happened during an agentic
/// run, flattened onto the canonical `resilience.agent.*` / `gen_ai.*` schema
/// via `span_attributes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelemetryEvidence {
    /// Identifier of the experiment this evidence belongs to.
    pub experiment_id: String,
    /// Identifier of the individual run, when known.
    pub run_id: Option<String>,
    /// Name of the chaos scenario executed.
    pub scenario: String,
    /// Type of fault applied to the agent.
    pub fault_type: String,
    /// The GenAI operation this evidence describes.
    pub operation: GenAiOperation,
    /// Name of the tool invoked, when the operation involved a tool.
    pub tool_name: Option<String>,
    /// The GenAI provider system (e.g. `anthropic`), when known.
    pub gen_ai_system: Option<String>,
    /// Model requested by the client, when known.
    pub request_model: Option<String>,
    /// Model that actually produced the response, when known.
    pub response_model: Option<String>,
    /// Outcome of the evaluation, when the operation was an evaluation.
    pub evaluation_result: Option<String>,
    /// Trace context linking this evidence to the `OTel` span tree, when available.
    pub trace: Option<AgenticTraceContext>,
    /// Size/hash metadata of the captured payload, when captured.
    pub payload: Option<PayloadMetadata>,
}

impl TelemetryEvidence {
    /// Create evidence with the required fields; all optional fields start unset.
    #[must_use]
    pub fn new(
        experiment_id: impl Into<String>,
        scenario: impl Into<String>,
        fault_type: impl Into<String>,
        operation: GenAiOperation,
    ) -> Self {
        Self {
            experiment_id: experiment_id.into(),
            run_id: None,
            scenario: scenario.into(),
            fault_type: fault_type.into(),
            operation,
            tool_name: None,
            gen_ai_system: None,
            request_model: None,
            response_model: None,
            evaluation_result: None,
            trace: None,
            payload: None,
        }
    }

    /// Set the run identifier.
    #[must_use]
    pub fn with_run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    /// Set the invoked tool name.
    #[must_use]
    pub fn with_tool_name(mut self, tool_name: impl Into<String>) -> Self {
        self.tool_name = Some(tool_name.into());
        self
    }

    /// Set the GenAI provider system.
    #[must_use]
    pub fn with_gen_ai_system(mut self, gen_ai_system: impl Into<String>) -> Self {
        self.gen_ai_system = Some(gen_ai_system.into());
        self
    }

    /// Set the model requested by the client.
    #[must_use]
    pub fn with_request_model(mut self, request_model: impl Into<String>) -> Self {
        self.request_model = Some(request_model.into());
        self
    }

    /// Set the model that produced the response.
    #[must_use]
    pub fn with_response_model(mut self, response_model: impl Into<String>) -> Self {
        self.response_model = Some(response_model.into());
        self
    }

    /// Set the evaluation outcome.
    #[must_use]
    pub fn with_evaluation_result(mut self, evaluation_result: impl Into<String>) -> Self {
        self.evaluation_result = Some(evaluation_result.into());
        self
    }

    /// Attach the trace context linking this evidence to the span tree.
    #[must_use]
    pub fn with_trace_context(mut self, trace: AgenticTraceContext) -> Self {
        self.trace = Some(trace);
        self
    }

    /// Attach payload size/hash metadata (metadata only, never the payload body).
    #[must_use]
    pub fn with_payload_metadata(
        mut self,
        input_bytes: u64,
        output_bytes: u64,
        payload_sha256: impl Into<String>,
    ) -> Self {
        self.payload = Some(PayloadMetadata {
            input_bytes,
            output_bytes,
            payload_sha256: payload_sha256.into(),
        });
        self
    }

    /// Flatten the evidence into `resilience.*` / `gen_ai.*` span attributes.
    /// Unset optional fields are omitted; numeric payload sizes are stringified.
    #[must_use]
    pub fn span_attributes(&self) -> BTreeMap<&'static str, String> {
        let mut attributes = BTreeMap::new();
        attributes.insert(RESILIENCE_EXPERIMENT_ID, self.experiment_id.clone());
        attributes.insert(RESILIENCE_AGENT_SCENARIO, self.scenario.clone());
        attributes.insert(RESILIENCE_AGENT_FAULT_TYPE, self.fault_type.clone());
        attributes.insert(RESILIENCE_AGENT_CAPTURE_POLICY, "metadata_only".to_string());
        attributes.insert(GEN_AI_OPERATION_NAME, self.operation.as_str().to_string());

        insert_optional(&mut attributes, RESILIENCE_RUN_ID, self.run_id.as_ref());
        insert_optional(&mut attributes, GEN_AI_TOOL_NAME, self.tool_name.as_ref());
        insert_optional(&mut attributes, GEN_AI_SYSTEM, self.gen_ai_system.as_ref());
        insert_optional(
            &mut attributes,
            GEN_AI_REQUEST_MODEL,
            self.request_model.as_ref(),
        );
        insert_optional(
            &mut attributes,
            GEN_AI_RESPONSE_MODEL,
            self.response_model.as_ref(),
        );
        insert_optional(
            &mut attributes,
            GEN_AI_EVALUATION_RESULT,
            self.evaluation_result.as_ref(),
        );

        if let Some(trace) = &self.trace {
            attributes.insert(RESILIENCE_TRACE_ID, trace.trace_id.clone());
            attributes.insert(RESILIENCE_SPAN_ID, trace.span_id.clone());
            insert_optional(
                &mut attributes,
                RESILIENCE_PARENT_SPAN_ID,
                trace.parent_span_id.as_ref(),
            );
        }

        if let Some(payload) = &self.payload {
            attributes.insert(
                RESILIENCE_AGENT_INPUT_BYTES,
                payload.input_bytes.to_string(),
            );
            attributes.insert(
                RESILIENCE_AGENT_OUTPUT_BYTES,
                payload.output_bytes.to_string(),
            );
            attributes.insert(
                RESILIENCE_AGENT_PAYLOAD_SHA256,
                payload.payload_sha256.clone(),
            );
        }

        attributes
    }
}

fn insert_optional(
    attributes: &mut BTreeMap<&'static str, String>,
    key: &'static str,
    value: Option<&String>,
) {
    if let Some(value) = value {
        attributes.insert(key, value.clone());
    }
}
