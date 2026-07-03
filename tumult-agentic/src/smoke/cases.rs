use super::helpers::{contract_actual, smoke_run_result};
use super::report::SmokeReport;
use crate::adapters::{
    fixture_response, AgentAdapter, AgentResponse, FakeHttpAgentAdapter, FakeMcpAdapter,
    McpToolInvocation,
};
use crate::contracts::{evaluate_contract, ContractSpec};
use crate::faults::FaultSpec;
use crate::model::{AgenticError, AgenticRunResult, AgenticScenario, FaultApplication};
use crate::replay::{
    complete_replay_fixture, incomplete_replay_fixture_missing_output_ref, ReplayAdapter,
    ReplayFixture,
};
use crate::scoring::resilience_score;

#[must_use]
pub fn malformed_json_smoke_result() -> AgenticRunResult {
    let response = AgentResponse {
        body: "{\"broken\":".to_string(),
        latency_ms: 12,
        tool_calls: 0,
        retry_count: 0,
        input_tokens: 5,
        output_tokens: 2,
        fallback_used: false,
    };
    let contract = ContractSpec::ValidJson {
        severity: Some(1.0),
    };
    let outcome = evaluate_contract("fake-http-malformed-json", &contract, &response);
    let fault = FaultSpec::MalformedOutput { probability: 1.0 };
    let contracts = vec![outcome];

    AgenticRunResult {
        target_type: "http".to_string(),
        scenarios: vec!["fake-http-malformed-json".to_string()],
        faults: vec![FaultApplication {
            fault_type: fault.fault_type().to_string(),
            scenario: "fake-http-malformed-json".to_string(),
            applied: true,
            started_at_ns: 1,
            ended_at_ns: 2,
        }],
        resilience_score: resilience_score(&contracts),
        contracts,
        trace_id: Some("trace-smoke".to_string()),
        replay_id: None,
    }
}

/// Run the deterministic fake HTTP malformed-output smoke case.
///
/// # Errors
///
/// Returns [`AgenticError`] only when the fake adapter unexpectedly fails.
pub fn fake_http_malformed_json_smoke() -> Result<SmokeReport, AgenticError> {
    let scenario = AgenticScenario {
        name: "fake-http-malformed-json".to_string(),
        input: "return a structured status object".to_string(),
        expected_behavior: Some("respond with valid JSON or a graceful fallback".to_string()),
    };
    let adapter = FakeHttpAgentAdapter::new("local-fixture", fixture_response("{\"broken\":"));
    let response = adapter.invoke(&scenario)?;
    let contract = ContractSpec::ValidJson {
        severity: Some(1.0),
    };
    let outcome = evaluate_contract(&scenario.name, &contract, &response);
    let run_result = smoke_run_result(
        "http",
        &scenario.name,
        "malformed_output",
        vec![outcome],
        Some("trace-smoke-http".to_string()),
        None,
    );

    Ok(SmokeReport {
        adapter: "fake_http".to_string(),
        scenario: scenario.name,
        fault: "malformed_output".to_string(),
        contract: "valid_json".to_string(),
        expected: "contract_failed:invalid_json".to_string(),
        actual: contract_actual(&run_result),
        next_diagnostic_command: "cargo test -p tumult-agentic smoke_fake_http -- --nocapture"
            .to_string(),
        passed: !run_result.contracts[0].passed
            && run_result.contracts[0].reason.as_deref() == Some("invalid_json"),
        run_result,
    })
}

/// Run the deterministic fake MCP tool failure smoke case.
///
/// # Errors
///
/// Returns [`AgenticError`] when the fake MCP fixture does not produce the
/// expected local failure path.
pub fn fake_mcp_tool_failure_smoke() -> Result<SmokeReport, AgenticError> {
    let scenario = AgenticScenario {
        name: "fake-mcp-tool-failure".to_string(),
        input: "look up the release status".to_string(),
        expected_behavior: Some("surface a graceful tool error".to_string()),
    };
    let adapter = FakeMcpAdapter::new("local-mcp", "lookup", fixture_response(r#"{"ok":true}"#))
        .with_failure("tool_unavailable");
    let invocation = McpToolInvocation {
        input: serde_json::json!({"scenario": scenario.name, "query": "release"}),
        required_fields: vec!["scenario".to_string(), "query".to_string()],
        trace_context: None,
    };
    let error = match adapter.invoke_tool(&invocation) {
        Ok(response) => {
            return Err(AgenticError::Adapter(format!(
                "adapter=fake_mcp scenario={} error=expected_failure actual_body={}",
                scenario.name, response.body
            )));
        }
        Err(error) => error,
    };
    let response = fixture_response(format!(r#"{{"error":"{error}"}}"#));
    let contract = ContractSpec::GracefulError {
        severity: Some(1.0),
    };
    let outcome = evaluate_contract(&scenario.name, &contract, &response);
    let run_result = smoke_run_result(
        "mcp",
        &scenario.name,
        "tool_failure",
        vec![outcome],
        Some("trace-smoke-mcp".to_string()),
        None,
    );

    Ok(SmokeReport {
        adapter: "fake_mcp".to_string(),
        scenario: scenario.name,
        fault: "tool_failure".to_string(),
        contract: "graceful_error".to_string(),
        expected: "contract_passed".to_string(),
        actual: contract_actual(&run_result),
        next_diagnostic_command: "cargo test -p tumult-agentic smoke_fake_mcp -- --nocapture"
            .to_string(),
        passed: run_result.contracts[0].passed,
        run_result,
    })
}

/// Run deterministic replay fixture smoke validation.
///
/// # Errors
///
/// Returns [`AgenticError`] when the complete fixture cannot be adapted or the
/// intentionally incomplete fixture is not rejected.
pub fn replay_validation_smoke() -> Result<SmokeReport, AgenticError> {
    let scenario = AgenticScenario {
        name: "replay-missing-output-ref".to_string(),
        input: "replay the captured session".to_string(),
        expected_behavior: Some("reject fixtures that omit output refs".to_string()),
    };
    let adapter = ReplayAdapter::new(complete_replay_fixture())?;
    let response = adapter.invoke(&scenario)?;
    let valid_json = ContractSpec::ValidJson {
        severity: Some(1.0),
    };
    let outcome = evaluate_contract(&scenario.name, &valid_json, &response);
    let missing_output_ref_rejected =
        ReplayAdapter::new(incomplete_replay_fixture_missing_output_ref()).is_err();
    let run_result = smoke_run_result(
        "replay",
        &scenario.name,
        "replay_validation",
        vec![outcome],
        Some("trace-smoke-replay".to_string()),
        Some("replay-smoke-001".to_string()),
    );

    Ok(SmokeReport {
        adapter: "replay".to_string(),
        scenario: scenario.name,
        fault: "replay_validation".to_string(),
        contract: "missing_output_ref".to_string(),
        expected: "incomplete_replay_rejected".to_string(),
        actual: if missing_output_ref_rejected {
            "incomplete_replay_rejected".to_string()
        } else {
            "incomplete_replay_accepted".to_string()
        },
        next_diagnostic_command: "cargo test -p tumult-agentic replay_validation -- --nocapture"
            .to_string(),
        passed: missing_output_ref_rejected && run_result.contracts[0].passed,
        run_result,
    })
}

/// Run all local smoke cases.
///
/// # Errors
///
/// Returns [`AgenticError`] when a fixture adapter unexpectedly errors.
pub fn run_local_smoke_suite() -> Result<Vec<SmokeReport>, AgenticError> {
    Ok(vec![
        fake_http_malformed_json_smoke()?,
        fake_mcp_tool_failure_smoke()?,
        replay_validation_smoke()?,
    ])
}

/// Replay a captured fixture through the real [`ReplayAdapter`].
///
/// Unlike [`replay_validation_smoke`], which exercises a built-in fixture, this
/// runs the caller-supplied fixture end to end: it is validated, every step is
/// replayed, and the resulting response is checked against the `ValidJson`
/// contract. The report's `replay_id` and trace echo the fixture's own
/// `session_id`.
///
/// # Errors
///
/// Returns [`AgenticError::IncompleteReplay`] when the fixture is missing steps
/// or output references, or [`AgenticError::Adapter`] if replay fails.
pub fn replay_fixture_smoke(fixture: ReplayFixture) -> Result<SmokeReport, AgenticError> {
    let session_id = fixture.session_id.clone();
    let source = fixture.source.clone();
    let step_count = fixture.steps.len();

    let scenario = AgenticScenario {
        name: format!("replay-{session_id}"),
        input: format!("replay {step_count} captured steps from {source}"),
        expected_behavior: Some("replay produces valid JSON for every captured step".to_string()),
    };

    let adapter = ReplayAdapter::new(fixture)?;
    let response = adapter.invoke(&scenario)?;
    let contract = ContractSpec::ValidJson {
        severity: Some(1.0),
    };
    let outcome = evaluate_contract(&scenario.name, &contract, &response);
    let run_result = smoke_run_result(
        "replay",
        &scenario.name,
        "replay_fixture",
        vec![outcome],
        Some(format!("trace-replay-{session_id}")),
        Some(session_id),
    );

    Ok(SmokeReport {
        adapter: "replay".to_string(),
        scenario: scenario.name,
        fault: "replay_fixture".to_string(),
        contract: "valid_json".to_string(),
        expected: "contract_passed".to_string(),
        actual: contract_actual(&run_result),
        next_diagnostic_command: "cargo test -p tumult-agentic replay_fixture -- --nocapture"
            .to_string(),
        passed: run_result.contracts[0].passed,
        run_result,
    })
}
