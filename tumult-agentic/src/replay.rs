use serde::{Deserialize, Serialize};

use crate::adapters::{fixture_response, AgentAdapter, AgentResponse};
use crate::model::{AgenticError, AgenticScenario};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayFixture {
    pub source: String,
    pub session_id: String,
    pub steps: Vec<ReplayStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReplayStep {
    ModelResponse {
        operation: String,
        output_ref: String,
    },
    ToolResult {
        tool_name: String,
        output_ref: String,
    },
    RetrievalResult {
        data_source: String,
        output_ref: String,
    },
}

impl ReplayFixture {
    /// Validate that replay steps are complete enough to run locally.
    ///
    /// # Errors
    ///
    /// Returns [`AgenticError::IncompleteReplay`] when a fixture has no steps
    /// or a replay step omits its output reference.
    pub fn validate(&self) -> Result<(), AgenticError> {
        if self.steps.is_empty() {
            return Err(AgenticError::IncompleteReplay(format!(
                "source={} session_id={} missing=steps",
                self.source, self.session_id
            )));
        }

        for (idx, step) in self.steps.iter().enumerate() {
            if step.output_ref().trim().is_empty() {
                return Err(AgenticError::IncompleteReplay(format!(
                    "source={} session_id={} step_index={idx} step_type={} missing=output_ref",
                    self.source,
                    self.session_id,
                    step.step_type()
                )));
            }
        }

        Ok(())
    }

    #[must_use]
    pub fn output_refs(&self) -> Vec<&str> {
        self.steps.iter().map(ReplayStep::output_ref).collect()
    }
}

impl ReplayStep {
    #[must_use]
    pub fn step_type(&self) -> &'static str {
        match self {
            Self::ModelResponse { .. } => "model_response",
            Self::ToolResult { .. } => "tool_result",
            Self::RetrievalResult { .. } => "retrieval_result",
        }
    }

    #[must_use]
    pub fn output_ref(&self) -> &str {
        match self {
            Self::ModelResponse { output_ref, .. }
            | Self::ToolResult { output_ref, .. }
            | Self::RetrievalResult { output_ref, .. } => output_ref,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReplayAdapter {
    fixture: ReplayFixture,
}

impl ReplayAdapter {
    /// Create a replay adapter after validating the fixture.
    ///
    /// # Errors
    ///
    /// Returns [`AgenticError::IncompleteReplay`] when the fixture is missing
    /// steps or any step output reference.
    pub fn new(fixture: ReplayFixture) -> Result<Self, AgenticError> {
        fixture.validate()?;
        Ok(Self { fixture })
    }

    #[must_use]
    pub fn fixture(&self) -> &ReplayFixture {
        &self.fixture
    }
}

impl AgentAdapter for ReplayAdapter {
    fn invoke(&self, scenario: &AgenticScenario) -> Result<AgentResponse, AgenticError> {
        let refs = self.fixture.output_refs().join(",");
        Ok(fixture_response(format!(
            r#"{{"scenario":"{}","replay_session":"{}","output_refs":"{}"}}"#,
            scenario.name, self.fixture.session_id, refs
        )))
    }
}

#[must_use]
pub fn complete_replay_fixture() -> ReplayFixture {
    ReplayFixture {
        source: "local-smoke".to_string(),
        session_id: "replay-smoke-001".to_string(),
        steps: vec![
            ReplayStep::ModelResponse {
                operation: "chat".to_string(),
                output_ref: "model-output-001".to_string(),
            },
            ReplayStep::ToolResult {
                tool_name: "lookup".to_string(),
                output_ref: "tool-output-001".to_string(),
            },
            ReplayStep::RetrievalResult {
                data_source: "docs".to_string(),
                output_ref: "retrieval-output-001".to_string(),
            },
        ],
    }
}

#[must_use]
pub fn incomplete_replay_fixture_missing_output_ref() -> ReplayFixture {
    ReplayFixture {
        source: "local-smoke".to_string(),
        session_id: "replay-smoke-missing-output".to_string(),
        steps: vec![ReplayStep::ModelResponse {
            operation: "chat".to_string(),
            output_ref: String::new(),
        }],
    }
}
