use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum AgenticError {
    #[error("unsupported agentic target type: {0}")]
    UnsupportedTarget(String),
    #[error("unsupported agentic fault type: {0}")]
    UnsupportedFault(String),
    #[error("target is not allowed: {0}")]
    TargetNotAllowed(String),
    #[error("invalid agentic configuration: {0}")]
    InvalidConfig(String),
    #[error("adapter error: {0}")]
    Adapter(String),
    #[error("contract failed: {0}")]
    Contract(String),
    #[error("replay fixture is incomplete: {0}")]
    IncompleteReplay(String),
    #[error("journal error: {0}")]
    Journal(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgenticTarget {
    Http { endpoint: String },
    Mcp { server: String, tool: String },
    Replay { fixture: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgenticScenario {
    pub name: String,
    pub input: String,
    pub expected_behavior: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgenticExperiment {
    pub target: AgenticTarget,
    pub scenarios: Vec<AgenticScenario>,
    pub faults: Vec<crate::faults::FaultSpec>,
    pub contracts: Vec<crate::contracts::ContractSpec>,
    pub privacy: PrivacyConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivacyConfig {
    pub capture_policy: CapturePolicy,
    pub target_allowlist: Vec<String>,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            capture_policy: CapturePolicy::MetadataOnly,
            target_allowlist: Vec::new(),
        }
    }
}

/// Validate an agentic experiment before execution.
///
/// # Errors
///
/// Returns [`AgenticError::InvalidConfig`] when the experiment matrix is empty
/// or a fault probability is outside the inclusive range `0.0..=1.0`.
pub fn validate_experiment(experiment: &AgenticExperiment) -> Result<(), AgenticError> {
    if experiment.scenarios.is_empty() {
        return Err(AgenticError::InvalidConfig(
            "at least one scenario is required".to_string(),
        ));
    }
    if experiment.faults.is_empty() {
        return Err(AgenticError::InvalidConfig(
            "at least one fault is required".to_string(),
        ));
    }
    if experiment.contracts.is_empty() {
        return Err(AgenticError::InvalidConfig(
            "at least one contract is required".to_string(),
        ));
    }

    for fault in &experiment.faults {
        let probability = fault.probability();
        if !(0.0..=1.0).contains(&probability) {
            return Err(AgenticError::InvalidConfig(format!(
                "fault {} probability must be between 0.0 and 1.0",
                fault.fault_type()
            )));
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapturePolicy {
    MetadataOnly,
    RedactedContent,
    RawContent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultApplication {
    pub fault_type: String,
    pub scenario: String,
    pub applied: bool,
    pub started_at_ns: i64,
    pub ended_at_ns: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContractOutcome {
    pub contract_type: String,
    pub scenario: String,
    pub passed: bool,
    pub reason: Option<String>,
    pub severity: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgenticRunResult {
    pub target_type: String,
    pub scenarios: Vec<String>,
    pub faults: Vec<FaultApplication>,
    pub contracts: Vec<ContractOutcome>,
    pub resilience_score: f64,
    pub trace_id: Option<String>,
    pub replay_id: Option<String>,
}
