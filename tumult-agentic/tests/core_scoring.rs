use tumult_agentic::model::ContractOutcome;
use tumult_agentic::scoring::{evaluate_score_gate, score_matrix, ScoreDimension};

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

    assert_eq!(score.overall, 0.5);
    assert_eq!(score.subscore(ScoreDimension::Latency), Some(1.0));
    assert_eq!(score.subscore(ScoreDimension::RetryBudget), Some(0.0));
    assert_eq!(score.subscore(ScoreDimension::Cost), Some(1.0));
    assert_eq!(score.subscore(ScoreDimension::Recovery), Some(1.0));
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
