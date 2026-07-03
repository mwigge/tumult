use serde::{Deserialize, Serialize};

use crate::model::{AgenticError, AgenticTarget, PrivacyConfig};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterSmokeExpectation {
    pub adapter: String,
    pub scenario: String,
    pub fault: String,
    pub contract: String,
    pub expected: String,
    pub actual: String,
    pub next_diagnostic_command: String,
}

impl AdapterSmokeExpectation {
    #[must_use]
    pub fn failure_message(&self) -> String {
        format!(
            "adapter={} scenario={} fault={} contract={} expected={} actual={} next_diagnostic_command={}",
            self.adapter,
            self.scenario,
            self.fault,
            self.contract,
            self.expected,
            self.actual,
            self.next_diagnostic_command
        )
    }
}

#[must_use]
pub fn adapter_failure_expectation(
    adapter: impl Into<String>,
    scenario: impl Into<String>,
    fault: impl Into<String>,
    contract: impl Into<String>,
    expected: impl Into<String>,
    actual: impl Into<String>,
    next_diagnostic_command: impl Into<String>,
) -> AdapterSmokeExpectation {
    AdapterSmokeExpectation {
        adapter: adapter.into(),
        scenario: scenario.into(),
        fault: fault.into(),
        contract: contract.into(),
        expected: expected.into(),
        actual: actual.into(),
        next_diagnostic_command: next_diagnostic_command.into(),
    }
}

#[must_use]
pub fn target_type(target: &AgenticTarget) -> &'static str {
    match target {
        AgenticTarget::Http { .. } => "http",
        AgenticTarget::Mcp { .. } => "mcp",
        AgenticTarget::Replay { .. } => "replay",
    }
}

/// # Errors
///
/// Returns [`AgenticError::TargetNotAllowed`] when the target endpoint,
/// server, or fixture is not covered by the configured allowlist.
pub fn validate_target(
    target: &AgenticTarget,
    privacy: &PrivacyConfig,
) -> Result<(), AgenticError> {
    let value = match target {
        AgenticTarget::Http { endpoint } => endpoint,
        AgenticTarget::Mcp { server, .. } => server,
        AgenticTarget::Replay { fixture } => fixture,
    };

    if privacy.target_allowlist.is_empty()
        || privacy
            .target_allowlist
            .iter()
            .any(|allowed| value.starts_with(allowed))
    {
        return Ok(());
    }

    Err(AgenticError::TargetNotAllowed(value.clone()))
}
