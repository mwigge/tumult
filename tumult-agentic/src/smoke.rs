use crate::adapters::{
    adapter_failure_expectation, fixture_response, AdapterSmokeExpectation, AgentAdapter,
    AgentResponse, FakeHttpAgentAdapter, FakeMcpAdapter, McpToolInvocation,
};
use crate::contracts::{evaluate_contract, ContractSpec};
use crate::engine::execute;
use crate::faults::{FaultSpec, FaultTargetResponse};
use crate::model::{AgenticError, AgenticRunResult, AgenticScenario, FaultApplication};
use crate::replay::{
    complete_replay_fixture, incomplete_replay_fixture_missing_output_ref, ReplayAdapter,
    ReplayFixture,
};
use crate::scenarios::bundled_packs;
use crate::scoring::resilience_score;

/// Fixed seed so local scenario-pack runs are reproducible.
const SCENARIO_PACK_SEED: u64 = 0x5eed;

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

/// The headline (fault, contract, expected-outcome) a scenario pack is built
/// to demonstrate. The pack also exercises its other faults/contracts, but the
/// `SmokeReport` summary surfaces this primary signal.
struct PackHeadline {
    fault: &'static str,
    contract: &'static str,
    expected: &'static str,
}

/// Run one bundled scenario pack through the real fault-execution engine.
///
/// Every fault in the pack is gated by the seeded [`crate::faults::FaultEngine`]
/// and applied via [`crate::faults::apply_fault`] against a per-pack baseline
/// response; every contract in the pack is then evaluated against the resulting
/// response. The returned [`SmokeReport`] surfaces the pack's headline
/// fault/contract pair while the embedded [`AgenticRunResult`] carries the full
/// fault and contract evidence.
///
/// # Errors
///
/// Returns [`AgenticError::InvalidConfig`] when the scenario pack is unknown,
/// or propagates [`AgenticError`] from fault application.
pub fn run_scenario_pack_smoke(scenario_pack: &str) -> Result<SmokeReport, AgenticError> {
    let pack = bundled_packs()
        .into_iter()
        .find(|pack| pack.name == scenario_pack)
        .ok_or_else(|| {
            AgenticError::InvalidConfig(format!("unknown scenario pack: {scenario_pack}"))
        })?;

    let target_type = pack.supported_adapters.first().copied().unwrap_or("http");
    let headline = pack_headline(pack.name);
    let baseline = pack_baseline(pack.name);

    let context = crate::engine::RunContext {
        target_type,
        scenario: pack.name,
        seed: SCENARIO_PACK_SEED,
        trace_id: Some(format!("trace-pack-{}", pack.name)),
        replay_id: None,
    };
    let executed = execute(&context, baseline, &pack.faults, &pack.contracts)?;
    let run_result = executed.result;

    let actual = run_result
        .contracts
        .iter()
        .find(|outcome| outcome.contract_type == headline.contract)
        .map_or_else(|| "contract_missing".to_string(), contract_outcome_actual);

    Ok(SmokeReport {
        adapter: format!("fake_{target_type}"),
        scenario: pack.name.to_string(),
        fault: headline.fault.to_string(),
        contract: headline.contract.to_string(),
        expected: headline.expected.to_string(),
        actual: actual.clone(),
        next_diagnostic_command: format!(
            "cargo test -p tumult-agentic {} -- --nocapture",
            pack.name
        ),
        passed: actual == headline.expected,
        run_result,
    })
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

#[must_use]
pub fn smoke_failure_output(report: &SmokeReport) -> Option<String> {
    if report.passed {
        None
    } else {
        Some(report.feedback_line())
    }
}

/// The per-pack headline fault/contract pair surfaced in the smoke report.
fn pack_headline(scenario_pack: &str) -> PackHeadline {
    match scenario_pack {
        "concurrency-storm" => PackHeadline {
            fault: "retry_loop_pressure",
            contract: "retry_budget",
            expected: "contract_failed:retry_budget_exceeded",
        },
        "hallucination-under-timeout" => PackHeadline {
            fault: "hallucinated_tool_call",
            contract: "max_tool_calls",
            expected: "contract_failed:tool_call_budget_exceeded",
        },
        "cost-explosion-detector" => PackHeadline {
            fault: "token_budget_exhaustion",
            contract: "max_token_usage",
            expected: "contract_failed:token_budget_exceeded",
        },
        "tool-timeout-fallback" => PackHeadline {
            fault: "tool_failure",
            contract: "fallback_used",
            expected: "contract_failed:fallback_not_used",
        },
        "retrieval-poisoning" => PackHeadline {
            fault: "retrieval_poisoning",
            contract: "required_citation",
            expected: "contract_failed:citation_missing",
        },
        // malformed-json-recovery and any future pack default to the
        // validity contract, which the malformed-output fault breaks.
        _ => PackHeadline {
            fault: "malformed_output",
            contract: "valid_json",
            expected: "contract_failed:invalid_json",
        },
    }
}

/// A per-pack baseline response chosen so the pack's faults, when applied by
/// the real engine, drive the pack's headline contract to its documented
/// outcome. A single universal baseline cannot do this: e.g. the hallucination
/// pack needs a legitimate in-flight tool call so one extra hallucinated call
/// trips the `max_tool_calls: 1` budget, while the cost pack needs a
/// token-heavy request that exceeds the `max_token_usage: 512` budget.
fn pack_baseline(scenario_pack: &str) -> FaultTargetResponse {
    let mut baseline = FaultTargetResponse {
        body: r#"{"status":"ok"}"#.to_string(),
        latency_ms: 50,
        retry_count: 0,
        tool_calls: 0,
        input_tokens: 16,
        output_tokens: 16,
        fallback_used: false,
        tool_name: None,
        retrieved_documents: Vec::new(),
    };

    match scenario_pack {
        "hallucination-under-timeout" => {
            // One legitimate tool call is already in flight; the hallucinated
            // call pushes the total past the max_tool_calls budget of 1.
            baseline.tool_calls = 1;
            baseline.tool_name = Some("lookup_order".to_string());
        }
        "cost-explosion-detector" => {
            // A large prompt already exceeds the 512-token cost budget, so even
            // after budget-exhaustion truncation the run is flagged as costly.
            baseline.input_tokens = 600;
            baseline.output_tokens = 200;
        }
        "tool-timeout-fallback" => {
            baseline.tool_name = Some("lookup_order".to_string());
        }
        "retrieval-poisoning" => {
            baseline.body = r#"{"answer":"a confident but unsourced answer"}"#.to_string();
        }
        _ => {}
    }

    baseline
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
    contract_outcome_actual(outcome)
}

fn contract_outcome_actual(outcome: &crate::model::ContractOutcome) -> String {
    if outcome.passed {
        "contract_passed".to_string()
    } else {
        format!(
            "contract_failed:{}",
            outcome.reason.as_deref().unwrap_or("unknown")
        )
    }
}
