use super::helpers::contract_outcome_actual;
use super::report::SmokeReport;
use crate::engine::{execute, RunContext};
use crate::faults::FaultTargetResponse;
use crate::model::AgenticError;
use crate::scenarios::bundled_packs;

/// Fixed seed so local scenario-pack runs are reproducible.
const SCENARIO_PACK_SEED: u64 = 0x5eed;

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

    let context = RunContext {
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
