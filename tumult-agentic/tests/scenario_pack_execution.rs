//! Proves the bundled scenario packs run through the *real* fault-execution
//! engine (`FaultEngine` gate + `apply_fault` mutation + contract evaluation)
//! rather than hand-scripted post-fault literals, and that each pack's faults
//! genuinely drive its headline contract to the documented outcome.

use tumult_agentic::contracts::ContractSpec;
use tumult_agentic::engine::{execute, RunContext};
use tumult_agentic::faults::{FaultSpec, FaultTargetResponse};
use tumult_agentic::scenarios::bundled_packs;
use tumult_agentic::smoke::run_scenario_pack_smoke;

fn baseline() -> FaultTargetResponse {
    FaultTargetResponse {
        body: r#"{"status":"ok"}"#.to_string(),
        latency_ms: 50,
        retry_count: 0,
        tool_calls: 0,
        input_tokens: 16,
        output_tokens: 16,
        fallback_used: false,
        tool_name: None,
        retrieved_documents: Vec::new(),
    }
}

#[test]
fn every_pack_applies_all_faults_and_evaluates_all_contracts() {
    for pack in bundled_packs() {
        let report = run_scenario_pack_smoke(pack.name)
            .unwrap_or_else(|err| panic!("pack {} should run: {err}", pack.name));

        assert_eq!(
            report.run_result.faults.len(),
            pack.faults.len(),
            "{} should record every fault",
            pack.name
        );
        assert!(
            report.run_result.faults.iter().all(|fault| fault.applied),
            "{} faults all have probability 1.0 so all must apply",
            pack.name
        );
        assert_eq!(
            report.run_result.contracts.len(),
            pack.contracts.len(),
            "{} should evaluate every contract",
            pack.name
        );
        assert!(
            report.passed,
            "{} headline contract should reach its documented outcome (actual={})",
            pack.name, report.actual
        );
    }
}

#[test]
fn concurrency_storm_retry_pressure_breaks_retry_budget() {
    let report = run_scenario_pack_smoke("concurrency-storm").expect("runs");
    assert_eq!(report.fault, "retry_loop_pressure");
    assert_eq!(report.contract, "retry_budget");
    assert_eq!(report.actual, "contract_failed:retry_budget_exceeded");
}

#[test]
fn hallucination_pack_trips_tool_call_budget() {
    let report = run_scenario_pack_smoke("hallucination-under-timeout").expect("runs");
    assert_eq!(report.actual, "contract_failed:tool_call_budget_exceeded");
}

#[test]
fn cost_pack_trips_token_budget() {
    let report = run_scenario_pack_smoke("cost-explosion-detector").expect("runs");
    assert_eq!(report.actual, "contract_failed:token_budget_exceeded");
}

#[test]
fn malformed_pack_trips_valid_json() {
    let report = run_scenario_pack_smoke("malformed-json-recovery").expect("runs");
    assert_eq!(report.actual, "contract_failed:invalid_json");
}

#[test]
fn tool_timeout_pack_trips_fallback() {
    let report = run_scenario_pack_smoke("tool-timeout-fallback").expect("runs");
    assert_eq!(report.actual, "contract_failed:fallback_not_used");
}

#[test]
fn retrieval_poisoning_contaminates_the_evaluated_body() {
    // Run the poisoning fault directly through the engine so we can inspect the
    // post-fault response the contracts actually scored. The poison must land
    // in the body, not only in retrieved_documents, or the body-based safety
    // contracts would never observe it.
    let context = RunContext {
        target_type: "http",
        scenario: "retrieval-poisoning",
        seed: 0x5eed,
        trace_id: None,
        replay_id: None,
    };
    let executed = execute(
        &context,
        baseline(),
        &[FaultSpec::RetrievalPoisoning {
            document_count: 2,
            probability: 1.0,
        }],
        &[ContractSpec::RequiredCitation {
            severity: Some(0.75),
        }],
    )
    .expect("poisoning runs");

    assert!(
        executed.response.body.contains("poisoned-document-0"),
        "poison must contaminate the evaluated body, got: {}",
        executed.response.body
    );
    assert!(
        executed.response.retrieved_documents.len() == 2,
        "poison must also be recorded in retrieved_documents"
    );
    assert!(
        !executed.result.contracts[0].passed,
        "uncited poisoned answer must fail the citation contract"
    );
}

#[test]
fn unknown_pack_is_rejected() {
    let err = run_scenario_pack_smoke("does-not-exist").expect_err("unknown pack must error");
    assert!(err.to_string().contains("unknown scenario pack"));
}
