//! Intelligence tools for agent reasoning: bundled agentic scenario smoke runs.

use crate::error::ToolError;

#[derive(Debug, serde::Serialize)]
struct AgenticSmokeReport {
    status: String,
    adapter: String,
    scenario: String,
    fault: String,
    contract: String,
    expected: String,
    actual: String,
    resilience_score: f64,
    raw_payloads_captured: bool,
    next_diagnostic_command: String,
}

/// Lists bundled agentic scenario packs without exposing prompt or payload data.
///
/// # Errors
///
/// Returns a [`ToolError`] if the scenario list cannot be serialized.
pub fn agentic_list_scenarios() -> Result<String, ToolError> {
    let packs = tumult_agentic::scenarios::bundled_packs()
        .into_iter()
        .map(|pack| {
            serde_json::json!({
                "name": pack.name,
                "adapters": pack.supported_adapters,
                "faults": pack.faults.iter().map(tumult_agentic::faults::FaultSpec::fault_type).collect::<Vec<_>>(),
                "contracts": pack.contracts.iter().map(tumult_agentic::contracts::ContractSpec::contract_type).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();

    serde_json::to_string_pretty(&serde_json::json!({
        "capture_policy": "metadata_only",
        "raw_payloads_captured": false,
        "packs": packs,
    }))
    .map_err(|e| ToolError::Execution(e.to_string()))
}

/// Runs a deterministic local agentic smoke scenario.
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] for unsupported adapters, scenarios,
/// faults, or contracts. The report never includes raw prompt or response bodies.
pub fn agentic_smoke(
    adapter: &str,
    scenario: &str,
    fault: Option<&str>,
    contract: Option<&str>,
) -> Result<String, ToolError> {
    if adapter != "fake-http" && adapter != "fake-mcp" && adapter != "replay" {
        return Err(ToolError::InvalidInput(format!(
            "unsupported agentic smoke adapter '{adapter}'; expected fake-http, fake-mcp, or replay"
        )));
    }

    let report = tumult_agentic::smoke::run_scenario_pack_smoke(scenario)
        .map_err(|err| ToolError::InvalidInput(err.to_string()))?;
    if let Some(selected_fault) = fault {
        if selected_fault != report.fault {
            return Err(ToolError::InvalidInput(format!(
                "fault '{selected_fault}' is not valid for scenario '{scenario}'"
            )));
        }
    }
    if let Some(selected_contract) = contract {
        if selected_contract != report.contract {
            return Err(ToolError::InvalidInput(format!(
                "contract '{selected_contract}' is not valid for scenario '{scenario}'"
            )));
        }
    }

    render_agentic_tool_report(
        report,
        "cargo test -p tumult-mcp agentic_smoke -- --nocapture",
    )
}

/// Runs a deterministic local agentic experiment from a bundled scenario pack.
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] for unsupported adapters, scenarios,
/// faults, or contracts. The report never includes raw prompt or response bodies.
pub fn agentic_run_experiment(
    adapter: &str,
    scenario: &str,
    fault: Option<&str>,
    contract: Option<&str>,
) -> Result<String, ToolError> {
    if adapter != "fake-http" && adapter != "fake-mcp" && adapter != "replay" {
        return Err(ToolError::InvalidInput(format!(
            "unsupported agentic run adapter '{adapter}'; expected fake-http, fake-mcp, or replay"
        )));
    }

    let report = tumult_agentic::smoke::run_scenario_pack_smoke(scenario)
        .map_err(|err| ToolError::InvalidInput(err.to_string()))?;
    if let Some(selected_fault) = fault {
        if selected_fault != report.fault {
            return Err(ToolError::InvalidInput(format!(
                "fault '{selected_fault}' is not valid for scenario '{scenario}'"
            )));
        }
    }
    if let Some(selected_contract) = contract {
        if selected_contract != report.contract {
            return Err(ToolError::InvalidInput(format!(
                "contract '{selected_contract}' is not valid for scenario '{scenario}'"
            )));
        }
    }

    render_agentic_tool_report(
        report,
        "cargo run -p tumult-cli -- agentic run --scenario <scenario>",
    )
}

fn render_agentic_tool_report(
    report: tumult_agentic::smoke::SmokeReport,
    next_diagnostic_command: &str,
) -> Result<String, ToolError> {
    let report = AgenticSmokeReport {
        status: if report.passed { "passed" } else { "failed" }.to_string(),
        adapter: report.adapter,
        scenario: report.scenario,
        fault: report.fault,
        contract: report.contract,
        expected: report.expected,
        actual: report.actual,
        resilience_score: report.run_result.resilience_score,
        raw_payloads_captured: false,
        next_diagnostic_command: next_diagnostic_command.to_string(),
    };

    serde_json::to_string_pretty(&report).map_err(|e| ToolError::Execution(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agentic_list_scenarios_returns_metadata_only_packs() {
        let output = agentic_list_scenarios().unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert_eq!(value["capture_policy"], "metadata_only");
        assert_eq!(value["raw_payloads_captured"], false);
        assert!(output.contains("malformed-json-recovery"));
        assert!(!output.contains("prompt"));
        assert!(!output.contains("customer secret"));
        assert!(!output.contains("\"input\""));
    }

    #[test]
    fn agentic_smoke_reports_clear_feedback_loop() {
        let output = agentic_smoke("fake-http", "malformed-json-recovery", None, None).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert_eq!(value["status"], "passed");
        assert_eq!(value["adapter"], "fake_http");
        // The scenario label is the bundled pack name now that the smoke runner
        // routes every pack through the real fault-execution engine.
        assert_eq!(value["scenario"], "malformed-json-recovery");
        assert_eq!(value["fault"], "malformed_output");
        assert_eq!(value["contract"], "valid_json");
        assert_eq!(value["expected"], "contract_failed:invalid_json");
        assert_eq!(value["actual"], "contract_failed:invalid_json");
        assert_eq!(value["raw_payloads_captured"], false);
        assert!(value["next_diagnostic_command"]
            .as_str()
            .unwrap()
            .contains("cargo test -p tumult-mcp agentic_smoke"));
    }

    #[test]
    fn agentic_smoke_validates_scenario_fault_and_contract() {
        assert!(agentic_smoke("real-http", "malformed-json-recovery", None, None).is_err());
        assert!(agentic_smoke("fake-http", "unknown", None, None).is_err());
        assert!(agentic_smoke(
            "fake-http",
            "malformed-json-recovery",
            Some("tool_timeout"),
            None
        )
        .is_err());
        assert!(agentic_smoke(
            "fake-http",
            "malformed-json-recovery",
            None,
            Some("fallback_used")
        )
        .is_err());
    }

    #[test]
    fn agentic_run_experiment_reports_metadata_only_feedback() {
        let output =
            agentic_run_experiment("fake-http", "cost-explosion-detector", None, None).unwrap();
        let value: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert_eq!(value["status"], "passed");
        assert_eq!(value["scenario"], "cost-explosion-detector");
        assert_eq!(value["fault"], "token_budget_exhaustion");
        assert_eq!(value["contract"], "max_token_usage");
        assert_eq!(value["raw_payloads_captured"], false);
        assert!(!output.contains("raw model output"));
    }
}
