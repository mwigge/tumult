use std::collections::BTreeMap;

use crate::model::ContractOutcome;

#[must_use]
pub fn resilience_score(outcomes: &[ContractOutcome]) -> f64 {
    let total: f64 = outcomes.iter().map(|outcome| outcome.severity).sum();
    if total == 0.0 {
        return 1.0;
    }

    let passed: f64 = outcomes
        .iter()
        .filter(|outcome| outcome.passed)
        .map(|outcome| outcome.severity)
        .sum();

    passed / total
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ScoreDimension {
    Latency,
    RetryBudget,
    Cost,
    Recovery,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScoreMatrix {
    pub overall: f64,
    subscores: BTreeMap<ScoreDimension, f64>,
}

impl ScoreMatrix {
    #[must_use]
    pub fn subscore(&self, dimension: ScoreDimension) -> Option<f64> {
        self.subscores.get(&dimension).copied()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScoreGateError {
    #[error(
        "resilience score gate failed: score {score:.3} below threshold {threshold:.3} by {delta:.3}; scenario={scenario} contract={contract} reason={reason}"
    )]
    BelowThreshold {
        score: f64,
        threshold: f64,
        delta: f64,
        scenario: String,
        contract: String,
        reason: String,
    },
}

#[must_use]
pub fn score_matrix(outcomes: &[ContractOutcome]) -> ScoreMatrix {
    let mut subscores = BTreeMap::new();
    insert_dimension(
        outcomes,
        &mut subscores,
        ScoreDimension::Latency,
        "max_latency",
    );
    insert_dimension(
        outcomes,
        &mut subscores,
        ScoreDimension::RetryBudget,
        "retry_budget",
    );
    insert_dimension(
        outcomes,
        &mut subscores,
        ScoreDimension::Cost,
        "max_token_usage",
    );
    insert_dimension(
        outcomes,
        &mut subscores,
        ScoreDimension::Recovery,
        "fallback_used",
    );

    ScoreMatrix {
        overall: resilience_score(outcomes),
        subscores,
    }
}

/// # Errors
///
/// Returns [`ScoreGateError::BelowThreshold`] when the overall resilience score
/// is below the requested threshold.
pub fn evaluate_score_gate(
    score: &ScoreMatrix,
    outcomes: &[ContractOutcome],
    threshold: f64,
) -> Result<(), ScoreGateError> {
    if score.overall >= threshold {
        return Ok(());
    }

    let failed = outcomes
        .iter()
        .find(|outcome| !outcome.passed)
        .cloned()
        .unwrap_or_else(|| ContractOutcome {
            contract_type: "unknown".to_string(),
            scenario: "unknown".to_string(),
            passed: false,
            reason: Some("score_below_threshold".to_string()),
            severity: 1.0,
        });

    Err(ScoreGateError::BelowThreshold {
        score: score.overall,
        threshold,
        delta: threshold - score.overall,
        scenario: failed.scenario,
        contract: failed.contract_type,
        reason: failed
            .reason
            .unwrap_or_else(|| "contract_failed".to_string()),
    })
}

fn insert_dimension(
    outcomes: &[ContractOutcome],
    subscores: &mut BTreeMap<ScoreDimension, f64>,
    dimension: ScoreDimension,
    contract_type: &str,
) {
    let matching = outcomes
        .iter()
        .filter(|outcome| outcome.contract_type == contract_type)
        .cloned()
        .collect::<Vec<_>>();
    if !matching.is_empty() {
        subscores.insert(dimension, resilience_score(&matching));
    }
}

/// A per-dimension agentic resilience subscore for a *multi-turn* trajectory.
///
/// Where [`ScoreDimension`] scores a single call's operational envelope, these
/// dimensions score how an agent trajectory behaves under a fault: does it
/// recover, keep its cost/step budget, stay correct, and avoid loops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AgenticDimension {
    /// Recovery after a bad step and healthy termination.
    Recovery,
    /// Bounded trajectory length / step budget.
    CostControl,
    /// Per-step contract correctness under the injected fault.
    CorrectnessUnderFault,
    /// Absence of repeated/looping steps.
    LoopAvoidance,
}

impl AgenticDimension {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Recovery => "recovery",
            Self::CostControl => "cost_control",
            Self::CorrectnessUnderFault => "correctness_under_fault",
            Self::LoopAvoidance => "loop_avoidance",
        }
    }
}

/// Rolled-up agentic resilience score for a trajectory run: an overall figure
/// plus the per-dimension subscores that explain it.
#[derive(Debug, Clone, PartialEq)]
pub struct AgenticScore {
    pub overall: f64,
    subscores: BTreeMap<AgenticDimension, f64>,
}

impl AgenticScore {
    #[must_use]
    pub fn subscore(&self, dimension: AgenticDimension) -> Option<f64> {
        self.subscores.get(&dimension).copied()
    }

    /// The populated dimensions in a stable order.
    #[must_use]
    pub fn dimensions(&self) -> Vec<(AgenticDimension, f64)> {
        self.subscores
            .iter()
            .map(|(dimension, score)| (*dimension, *score))
            .collect()
    }
}

/// Compute agentic subscores from a trajectory's per-step and trajectory-level
/// contract outcomes.
///
/// The overall score is the severity-weighted pass rate across *all* outcomes
/// (mirroring [`resilience_score`]), while each subscore isolates the outcomes
/// that speak to one dimension: `CorrectnessUnderFault` from the per-step
/// contracts, `Recovery`/`CostControl`/`LoopAvoidance` from the matching
/// trajectory contracts.
#[must_use]
pub fn agentic_score(
    step_contracts: &[ContractOutcome],
    trajectory_contracts: &[ContractOutcome],
) -> AgenticScore {
    let mut subscores = BTreeMap::new();

    if !step_contracts.is_empty() {
        subscores.insert(
            AgenticDimension::CorrectnessUnderFault,
            resilience_score(step_contracts),
        );
    }
    insert_agentic_dimension(
        trajectory_contracts,
        &mut subscores,
        AgenticDimension::Recovery,
        &["recovers_within", "terminates_healthy"],
    );
    insert_agentic_dimension(
        trajectory_contracts,
        &mut subscores,
        AgenticDimension::CostControl,
        &["step_budget"],
    );
    insert_agentic_dimension(
        trajectory_contracts,
        &mut subscores,
        AgenticDimension::LoopAvoidance,
        &["no_repeated_step"],
    );

    let mut combined = Vec::with_capacity(step_contracts.len() + trajectory_contracts.len());
    combined.extend(step_contracts.iter().cloned());
    combined.extend(trajectory_contracts.iter().cloned());

    AgenticScore {
        overall: resilience_score(&combined),
        subscores,
    }
}

fn insert_agentic_dimension(
    outcomes: &[ContractOutcome],
    subscores: &mut BTreeMap<AgenticDimension, f64>,
    dimension: AgenticDimension,
    contract_types: &[&str],
) {
    let matching = outcomes
        .iter()
        .filter(|outcome| contract_types.contains(&outcome.contract_type.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !matching.is_empty() {
        subscores.insert(dimension, resilience_score(&matching));
    }
}
