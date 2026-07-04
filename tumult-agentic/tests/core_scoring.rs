use tumult_agentic::model::ContractOutcome;
use tumult_agentic::scoring::{
    agentic_score, evaluate_score_gate, score_matrix, AgenticDimension, ScoreDimension,
};

fn outcome(contract_type: &str, passed: bool, severity: f64) -> ContractOutcome {
    ContractOutcome {
        contract_type: contract_type.to_string(),
        scenario: "support-order-lookup".to_string(),
        passed,
        reason: if passed {
            None
        } else {
            Some(format!("{contract_type}_failed"))
        },
        severity,
    }
}

#[test]
fn score_matrix_computes_weighted_resilience_and_subscores() {
    let outcomes = vec![
        outcome("valid_json", false, 3.0),
        outcome("max_latency", true, 1.0),
        outcome("retry_budget", false, 1.0),
        outcome("max_token_usage", true, 1.0),
        outcome("fallback_used", true, 2.0),
    ];

    let score = score_matrix(&outcomes);

    assert!((score.overall - 0.5).abs() < f64::EPSILON);
    assert_eq!(score.subscore(ScoreDimension::Latency), Some(1.0));
    assert_eq!(score.subscore(ScoreDimension::RetryBudget), Some(0.0));
    assert_eq!(score.subscore(ScoreDimension::Cost), Some(1.0));
    assert_eq!(score.subscore(ScoreDimension::Recovery), Some(1.0));
}

#[test]
fn agentic_score_rolls_up_trajectory_subscores_by_dimension() {
    // Per-step correctness: one of two step contracts failed → 0.5.
    let step_contracts = vec![
        outcome("valid_json", true, 1.0),
        outcome("required_citation", false, 1.0),
    ];
    // Trajectory contracts: recovered + terminated healthy, but a loop and an
    // over-budget length.
    let trajectory_contracts = vec![
        outcome("recovers_within", true, 1.0),
        outcome("terminates_healthy", true, 1.0),
        outcome("no_repeated_step", false, 1.0),
        outcome("step_budget", false, 1.0),
    ];

    let score = agentic_score(&step_contracts, &trajectory_contracts);

    assert_eq!(
        score.subscore(AgenticDimension::CorrectnessUnderFault),
        Some(0.5)
    );
    assert_eq!(score.subscore(AgenticDimension::Recovery), Some(1.0));
    assert_eq!(score.subscore(AgenticDimension::LoopAvoidance), Some(0.0));
    assert_eq!(score.subscore(AgenticDimension::CostControl), Some(0.0));
    // Overall = severity-weighted pass rate across all six outcomes = 3/6.
    assert!((score.overall - 0.5).abs() < f64::EPSILON);
}

#[test]
fn agentic_score_with_no_step_contracts_omits_correctness_dimension() {
    let trajectory_contracts = vec![outcome("terminates_healthy", true, 1.0)];
    let score = agentic_score(&[], &trajectory_contracts);
    assert_eq!(
        score.subscore(AgenticDimension::CorrectnessUnderFault),
        None
    );
    assert_eq!(score.subscore(AgenticDimension::Recovery), Some(1.0));
    assert!((score.overall - 1.0).abs() < f64::EPSILON);
}

#[test]
fn score_gate_failure_names_scenario_contract_and_delta() {
    let outcomes = vec![
        outcome("valid_json", false, 3.0),
        outcome("max_latency", true, 1.0),
    ];
    let score = score_matrix(&outcomes);

    let failure = evaluate_score_gate(&score, &outcomes, 0.90)
        .expect_err("expected score below threshold to produce CLI-style gate failure feedback");

    assert_eq!(
        failure.to_string(),
        "resilience score gate failed: score 0.250 below threshold 0.900 by 0.650; scenario=support-order-lookup contract=valid_json reason=valid_json_failed"
    );
}
