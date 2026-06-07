use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const RESILIENCE_EXPERIMENT_ID: &str = "resilience.experiment.id";
pub const RESILIENCE_RUN_ID: &str = "resilience.run.id";
pub const RESILIENCE_TRACE_ID: &str = "resilience.trace.id";
pub const RESILIENCE_SPAN_ID: &str = "resilience.span.id";
pub const RESILIENCE_PARENT_SPAN_ID: &str = "resilience.parent_span.id";
pub const RESILIENCE_AGENT_SCENARIO: &str = "resilience.agent.scenario";
pub const RESILIENCE_AGENT_FAULT_TYPE: &str = "resilience.agent.fault.type";
pub const RESILIENCE_AGENT_CONTRACT: &str = "resilience.agent.contract";
pub const RESILIENCE_AGENT_SCORE: &str = "resilience.agent.score";
pub const RESILIENCE_AGENT_CAPTURE_POLICY: &str = "resilience.agent.payload.capture_policy";
pub const RESILIENCE_AGENT_INPUT_BYTES: &str = "resilience.agent.input.bytes";
pub const RESILIENCE_AGENT_OUTPUT_BYTES: &str = "resilience.agent.output.bytes";
pub const RESILIENCE_AGENT_PAYLOAD_SHA256: &str = "resilience.agent.payload.sha256";
pub const GEN_AI_OPERATION_NAME: &str = "gen_ai.operation.name";
pub const GEN_AI_TOOL_NAME: &str = "gen_ai.tool.name";
pub const GEN_AI_EVALUATION_RESULT: &str = "gen_ai.evaluation.result";
pub const GEN_AI_REQUEST_MODEL: &str = "gen_ai.request.model";
pub const GEN_AI_RESPONSE_MODEL: &str = "gen_ai.response.model";
pub const GEN_AI_SYSTEM: &str = "gen_ai.system";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenAiOperation {
    Chat,
    GenerateContent,
    ExecuteTool,
    Embeddings,
    InvokeAgent,
    InvokeWorkflow,
    Evaluate,
}

impl GenAiOperation {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgenticTraceContext {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
}

impl AgenticTraceContext {
    #[must_use]
    pub fn new(trace_id: impl Into<String>, span_id: impl Into<String>) -> Self {
        Self {
            trace_id: trace_id.into(),
            span_id: span_id.into(),
            parent_span_id: None,
        }
    }

    #[must_use]
    pub fn with_parent_span_id(mut self, parent_span_id: impl Into<String>) -> Self {
        self.parent_span_id = Some(parent_span_id.into());
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadMetadata {
    pub input_bytes: u64,
    pub output_bytes: u64,
    pub payload_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelemetryEvidence {
    pub experiment_id: String,
    pub run_id: Option<String>,
    pub scenario: String,
    pub fault_type: String,
    pub operation: GenAiOperation,
    pub tool_name: Option<String>,
    pub gen_ai_system: Option<String>,
    pub request_model: Option<String>,
    pub response_model: Option<String>,
    pub evaluation_result: Option<String>,
    pub trace: Option<AgenticTraceContext>,
    pub payload: Option<PayloadMetadata>,
}

impl TelemetryEvidence {
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

    #[must_use]
    pub fn with_run_id(mut self, run_id: impl Into<String>) -> Self {
        self.run_id = Some(run_id.into());
        self
    }

    #[must_use]
    pub fn with_tool_name(mut self, tool_name: impl Into<String>) -> Self {
        self.tool_name = Some(tool_name.into());
        self
    }

    #[must_use]
    pub fn with_gen_ai_system(mut self, gen_ai_system: impl Into<String>) -> Self {
        self.gen_ai_system = Some(gen_ai_system.into());
        self
    }

    #[must_use]
    pub fn with_request_model(mut self, request_model: impl Into<String>) -> Self {
        self.request_model = Some(request_model.into());
        self
    }

    #[must_use]
    pub fn with_response_model(mut self, response_model: impl Into<String>) -> Self {
        self.response_model = Some(response_model.into());
        self
    }

    #[must_use]
    pub fn with_evaluation_result(mut self, evaluation_result: impl Into<String>) -> Self {
        self.evaluation_result = Some(evaluation_result.into());
        self
    }

    #[must_use]
    pub fn with_trace_context(mut self, trace: AgenticTraceContext) -> Self {
        self.trace = Some(trace);
        self
    }

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
