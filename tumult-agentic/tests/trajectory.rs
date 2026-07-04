//! Multi-turn trajectory modeling: fault-at-a-step injection, whole-trajectory
//! contracts, agentic subscores, and the bundled trajectory packs — all against
//! in-process metadata baselines (no network).

use tumult_agentic::contracts::ContractSpec;
use tumult_agentic::faults::{FaultSpec, FaultTargetResponse};
use tumult_agentic::scoring::AgenticDimension;
use tumult_agentic::smoke::run_trajectory_pack_smoke;
use tumult_agentic::trajectory::{
    bundled_trajectory_packs, execute_trajectory, StepFault, StepKind, TrajectoryContractSpec,
    TrajectoryStep,
};

fn base(body: &str) -> FaultTargetResponse {
    FaultTargetResponse {
        body: body.to_string(),
        latency_ms: 10,
        retry_count: 0,
        tool_calls: 1,
        input_tokens: 8,
        output_tokens: 8,
        fallback_used: false,
        tool_name: None,
        retrieved_documents: Vec::new(),
    }
}

#[test]
fn bundled_trajectory_packs_cover_the_agent_graph_catalog() {
    let packs = bundled_trajectory_packs();
    let names: Vec<_> = packs.iter().map(|pack| pack.name).collect();

    assert_eq!(packs.len(), 3);
    assert!(names.contains(&"rag-grounding-failure"));
    assert!(names.contains(&"reflection-loop"));
    assert!(names.contains(&"multi-tool-cascade"));

    for pack in packs {
        assert!(pack.steps.len() >= 2, "{} must be multi-turn", pack.name);
        assert!(!pack.faults.is_empty(), "{} must inject a fault", pack.name);
        assert!(
            !pack.contracts.is_empty(),
            "{} must have trajectory contracts",
            pack.name
        );
    }
}

#[test]
fn every_pack_smoke_reaches_its_documented_headline_outcome() {
    for pack in bundled_trajectory_packs() {
        let report = run_trajectory_pack_smoke(pack.name)
            .unwrap_or_else(|err| panic!("pack {} should run: {err}", pack.name));
        assert!(
            report.passed,
            "{} headline should reach {} (actual={})",
            pack.name, report.expected, report.actual
        );
        assert_eq!(report.result.steps.len(), pack.steps.len());
    }
}

#[test]
fn rag_grounding_failure_fault_at_step0_fails_a_contract_at_step2() {
    let report = run_trajectory_pack_smoke("rag-grounding-failure").expect("runs");

    // The fault is injected at the retrieve step (index 0)...
    assert_eq!(report.injected.len(), 1);
    assert_eq!(report.injected[0].step_index, 0);
    assert_eq!(report.injected[0].fault_type, "retrieval_poisoning");

    // ...but the answer step (index 2) is the one that loses its grounding.
    let answer = &report.result.steps[2];
    assert_eq!(answer.label, "answer");
    assert!(
        !answer.healthy,
        "poisoned context must break the answer step"
    );
    assert!(answer
        .contracts
        .iter()
        .any(|c| c.contract_type == "required_citation" && !c.passed));

    assert_eq!(
        report.actual,
        "trajectory_contract_failed:final_step_unhealthy"
    );
    // Recovery collapses, loop-avoidance intact.
    assert_eq!(
        report.result.score.subscore(AgenticDimension::Recovery),
        Some(0.0)
    );
    assert_eq!(
        report
            .result
            .score
            .subscore(AgenticDimension::LoopAvoidance),
        Some(1.0)
    );
}

#[test]
fn reflection_loop_is_detected_as_a_repeated_step() {
    let report = run_trajectory_pack_smoke("reflection-loop").expect("runs");

    assert_eq!(report.headline_contract, "no_repeated_step");
    assert_eq!(report.actual, "trajectory_contract_failed:loop_detected");
    // Step budget also trips (4 steps > budget 3).
    assert!(report
        .result
        .trajectory_contracts
        .iter()
        .any(|c| c.contract_type == "step_budget" && !c.passed));
    // Loop-avoidance subscore floored.
    assert_eq!(
        report
            .result
            .score
            .subscore(AgenticDimension::LoopAvoidance),
        Some(0.0)
    );
}

#[test]
fn multi_tool_cascade_recovers_via_fallback() {
    let report = run_trajectory_pack_smoke("multi-tool-cascade").expect("runs");

    // The tool failure lands on step 1...
    assert_eq!(report.injected[0].step_index, 1);
    assert!(!report.result.steps[1].healthy);
    // ...and the synthesize step recovers via a fallback.
    assert!(report.result.steps[2].healthy);
    assert_eq!(report.actual, "trajectory_contract_passed");
    assert_eq!(
        report.result.score.subscore(AgenticDimension::Recovery),
        Some(1.0)
    );
}

#[test]
fn recovers_within_fails_when_no_later_step_is_healthy() {
    // A trajectory that never recovers: a malformed-output fault at step 1 with
    // no healthy successor must fail RecoversWithin.
    let steps = vec![
        TrajectoryStep {
            label: "call".to_string(),
            kind: StepKind::Tool,
            consumes_retrieval: false,
            baseline: base(r#"{"ok":true}"#),
            contracts: vec![ContractSpec::ValidJson {
                severity: Some(1.0),
            }],
        },
        TrajectoryStep {
            label: "answer".to_string(),
            kind: StepKind::Model,
            consumes_retrieval: false,
            baseline: base(r#"{"answer":"x"}"#),
            contracts: vec![ContractSpec::ValidJson {
                severity: Some(1.0),
            }],
        },
    ];
    let faults = vec![StepFault {
        step_index: 1,
        fault: FaultSpec::MalformedOutput { probability: 1.0 },
    }];
    let contracts = vec![TrajectoryContractSpec::RecoversWithin {
        max_steps: 2,
        severity: Some(1.0),
    }];

    let result = execute_trajectory("no-recovery", &steps, &faults, &contracts, 1).expect("runs");
    assert!(!result.steps[1].healthy);
    assert_eq!(
        result.trajectory_contracts[0].contract_type,
        "recovers_within"
    );
    assert!(!result.trajectory_contracts[0].passed);
    assert_eq!(
        result.trajectory_contracts[0].reason.as_deref(),
        Some("did_not_recover")
    );
}

#[test]
fn fault_gating_is_deterministic_for_a_seed() {
    let steps = vec![TrajectoryStep {
        label: "call".to_string(),
        kind: StepKind::Model,
        consumes_retrieval: false,
        baseline: base(r#"{"ok":true}"#),
        contracts: vec![ContractSpec::ValidJson {
            severity: Some(1.0),
        }],
    }];
    let faults = vec![StepFault {
        step_index: 0,
        fault: FaultSpec::MalformedOutput { probability: 1.0 },
    }];
    let contracts = vec![TrajectoryContractSpec::TerminatesHealthy {
        severity: Some(1.0),
    }];

    let a = execute_trajectory("det", &steps, &faults, &contracts, 42).expect("runs");
    let b = execute_trajectory("det", &steps, &faults, &contracts, 42).expect("runs");
    assert_eq!(a, b);
}

#[test]
fn unknown_trajectory_pack_is_rejected() {
    let err = run_trajectory_pack_smoke("does-not-exist").expect_err("unknown pack errors");
    assert!(err.to_string().contains("unknown trajectory pack"));
}
