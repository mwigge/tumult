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
