use crate::adapters::{
    adapter_failure_expectation, fixture_response, AdapterSmokeExpectation, AgentAdapter,
    AgentResponse, FakeHttpAgentAdapter, FakeMcpAdapter, McpToolInvocation,
};
use crate::contracts::{evaluate_contract, ContractSpec};
use crate::faults::FaultSpec;
use crate::model::{AgenticError, AgenticRunResult, AgenticScenario, FaultApplication};
use crate::replay::{
    complete_replay_fixture, incomplete_replay_fixture_missing_output_ref, ReplayAdapter,
};
use crate::scenarios::bundled_packs;
use crate::scoring::resilience_score;

#[derive(Debug, Clone, PartialEq)]
pub struct SmokeReport {
    pub adapter: String,
    pub scenario: String,
    pub fault: String,
    pub contract: String,
    pub expected: String,
    pub actual: String,
    pub next_diagnostic_command: String,
    pub passed: bool,
    pub run_result: AgenticRunResult,
}

impl SmokeReport {
    #[must_use]
    pub fn feedback_line(&self) -> String {
        if self.passed {
            format!(
                "pass adapter={} scenario={} fault={} contract={} expected={} actual={} next_diagnostic_command={}",
                self.adapter,
                self.scenario,
                self.fault,
                self.contract,
                self.expected,
                self.actual,
                self.next_diagnostic_command
            )
        } else {
            self.expectation().failure_message()
        }
    }

    #[must_use]
    pub fn expectation(&self) -> AdapterSmokeExpectation {
        adapter_failure_expectation(
            self.adapter.clone(),
            self.scenario.clone(),
            self.fault.clone(),
            self.contract.clone(),
            self.expected.clone(),
            self.actual.clone(),
            self.next_diagnostic_command.clone(),
        )
    }
}

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

/// Run one bundled scenario pack through a deterministic local adapter.
///
/// # Errors
///
/// Returns [`AgenticError::InvalidConfig`] when the scenario pack is unknown.
/// Returns adapter errors if the underlying local fixture fails unexpectedly.
pub fn run_scenario_pack_smoke(scenario_pack: &str) -> Result<SmokeReport, AgenticError> {
    let exists = bundled_packs()
        .iter()
        .any(|pack| pack.name == scenario_pack);
    if !exists {
        return Err(AgenticError::InvalidConfig(format!(
            "unknown scenario pack: {scenario_pack}"
        )));
    }

    match scenario_pack {
        "malformed-json-recovery" => fake_http_malformed_json_smoke(),
        "tool-timeout-fallback" => fake_mcp_tool_failure_smoke(),
        "retrieval-poisoning" => Ok(scenario_contract_smoke(
            "http",
            "retrieval-poisoning",
            "retrieval_poisoning",
            &ContractSpec::RequiredCitation {
                severity: Some(0.75),
            },
            &fixture_response(r#"{"answer":"uncited degraded retrieval answer"}"#),
            "contract_failed:citation_missing",
        )),
        "concurrency-storm" => Ok(scenario_contract_smoke(
            "http",
            "concurrency-storm",
            "retry_loop_pressure",
            &ContractSpec::RetryBudget {
                max_retries: 2,
                severity: Some(1.0),
            },
            &AgentResponse {
                body: r#"{"status":"retry pressure"}"#.to_string(),
                latency_ms: 200,
                tool_calls: 0,
                retry_count: 5,
                input_tokens: 3,
                output_tokens: 3,
                fallback_used: false,
            },
            "contract_failed:retry_budget_exceeded",
        )),
        "hallucination-under-timeout" => Ok(scenario_contract_smoke(
            "mcp",
            "hallucination-under-timeout",
            "hallucinated_tool_call",
            &ContractSpec::MaxToolCalls {
                max_calls: 1,
                severity: Some(1.0),
            },
            &AgentResponse {
                body: r#"{"tool":"unknown_tool"}"#.to_string(),
                latency_ms: 1_500,
                tool_calls: 3,
                retry_count: 1,
                input_tokens: 4,
                output_tokens: 4,
                fallback_used: false,
            },
            "contract_failed:tool_call_budget_exceeded",
        )),
        "cost-explosion-detector" => Ok(scenario_contract_smoke(
            "http",
            "cost-explosion-detector",
            "token_budget_exhaustion",
            &ContractSpec::MaxTokenUsage {
                max_tokens: 512,
                severity: Some(1.0),
            },
            &AgentResponse {
                body: r#"{"status":"token budget exceeded"}"#.to_string(),
                latency_ms: 120,
                tool_calls: 0,
                retry_count: 4,
                input_tokens: 400,
                output_tokens: 300,
                fallback_used: false,
            },
            "contract_failed:token_budget_exceeded",
        )),
        _ => Err(AgenticError::InvalidConfig(format!(
            "unknown scenario pack: {scenario_pack}"
        ))),
    }
}

#[must_use]
pub fn smoke_failure_output(report: &SmokeReport) -> Option<String> {
    if report.passed {
        None
    } else {
        Some(report.feedback_line())
    }
}

fn scenario_contract_smoke(
    target_type: &str,
    scenario: &str,
    fault_type: &str,
    contract: &ContractSpec,
    response: &AgentResponse,
    expected: &str,
) -> SmokeReport {
    let outcome = evaluate_contract(scenario, contract, response);
    let run_result = smoke_run_result(
        target_type,
        scenario,
        fault_type,
        vec![outcome],
        Some(format!("trace-smoke-{scenario}")),
        None,
    );
    let actual = contract_actual(&run_result);
    let contract_type = contract.contract_type().to_string();
    SmokeReport {
        adapter: format!("fake_{target_type}"),
        scenario: scenario.to_string(),
        fault: fault_type.to_string(),
        contract: contract_type,
        expected: expected.to_string(),
        actual: actual.clone(),
        next_diagnostic_command: format!("cargo test -p tumult-agentic {scenario} -- --nocapture"),
        passed: actual == expected,
        run_result,
    }
}

fn smoke_run_result(
    target_type: &str,
    scenario: &str,
    fault_type: &str,
    contracts: Vec<crate::model::ContractOutcome>,
    trace_id: Option<String>,
    replay_id: Option<String>,
) -> AgenticRunResult {
    AgenticRunResult {
        target_type: target_type.to_string(),
        scenarios: vec![scenario.to_string()],
        faults: vec![FaultApplication {
            fault_type: fault_type.to_string(),
            scenario: scenario.to_string(),
            applied: true,
            started_at_ns: 1,
            ended_at_ns: 2,
        }],
        resilience_score: resilience_score(&contracts),
        contracts,
        trace_id,
        replay_id,
    }
}

fn contract_actual(run_result: &AgenticRunResult) -> String {
    let Some(outcome) = run_result.contracts.first() else {
        return "contract_missing".to_string();
    };

    if outcome.passed {
        "contract_passed".to_string()
    } else {
        format!(
            "contract_failed:{}",
            outcome.reason.as_deref().unwrap_or("unknown")
        )
    }
}
