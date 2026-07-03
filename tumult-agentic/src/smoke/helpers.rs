use crate::model::{AgenticRunResult, ContractOutcome, FaultApplication};
use crate::scoring::resilience_score;

pub(crate) fn smoke_run_result(
    target_type: &str,
    scenario: &str,
    fault_type: &str,
    contracts: Vec<ContractOutcome>,
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

pub(crate) fn contract_actual(run_result: &AgenticRunResult) -> String {
    let Some(outcome) = run_result.contracts.first() else {
        return "contract_missing".to_string();
    };
    contract_outcome_actual(outcome)
}

pub(crate) fn contract_outcome_actual(outcome: &ContractOutcome) -> String {
    if outcome.passed {
        "contract_passed".to_string()
    } else {
        format!(
            "contract_failed:{}",
            outcome.reason.as_deref().unwrap_or("unknown")
        )
    }
}
