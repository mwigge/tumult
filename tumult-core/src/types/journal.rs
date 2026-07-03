//! Experiment journal output types.

use serde::{Deserialize, Serialize};

use super::definition::{Estimate, Experiment, RegulatoryMapping};
use super::enums::{ActivityStatus, ActivityType, ExperimentStatus};
use super::ids::{SpanId, TraceId};
use super::results::{AnalysisResult, BaselineResult, DuringResult, LoadResult, PostResult};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityResult {
    pub name: String,
    pub activity_type: ActivityType,
    pub status: ActivityStatus,
    pub started_at_ns: i64,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub trace_id: TraceId,
    pub span_id: SpanId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HypothesisResult {
    pub title: String,
    pub met: bool,
    pub probe_results: Vec<ActivityResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    pub experiment_title: String,
    pub experiment_id: String,
    pub status: ExperimentStatus,
    pub started_at_ns: i64,
    pub ended_at_ns: i64,
    pub duration_ms: u64,
    pub steady_state_before: Option<HypothesisResult>,
    pub steady_state_after: Option<HypothesisResult>,
    pub method_results: Vec<ActivityResult>,
    pub rollback_results: Vec<ActivityResult>,
    /// Number of rollback activities that failed during execution.
    #[serde(default)]
    pub rollback_failures: u32,
    pub estimate: Option<Estimate>,
    pub baseline_result: Option<BaselineResult>,
    pub during_result: Option<DuringResult>,
    pub post_result: Option<PostResult>,
    pub load_result: Option<LoadResult>,
    pub analysis: Option<AnalysisResult>,
    pub regulatory: Option<RegulatoryMapping>,
}

impl Journal {
    /// Build a `Journal` skeleton for `experiment`: fills in the fields
    /// derived from the experiment definition (`experiment_title`,
    /// `estimate`, `regulatory`) plus the given identity/status/timing, and
    /// zeroes/empties every execution-result field. Callers use struct
    /// update syntax (`..Journal::for_experiment(...)`) to set whichever
    /// result fields apply to their completion path.
    pub(crate) fn for_experiment(
        experiment: &Experiment,
        experiment_id: String,
        status: ExperimentStatus,
        started_at_ns: i64,
    ) -> Self {
        Self {
            experiment_title: experiment.title.clone(),
            experiment_id,
            status,
            started_at_ns,
            ended_at_ns: started_at_ns,
            duration_ms: 0,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![],
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: experiment.estimate.clone(),
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: experiment.regulatory.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::types::test_support::toon_round_trip;
    use crate::types::*;

    #[test]
    fn activity_result_round_trips() {
        let result = ActivityResult {
            name: "kill-pod".into(),
            activity_type: ActivityType::Action,
            status: ActivityStatus::Succeeded,
            started_at_ns: 1_774_980_135_342_000_000,
            duration_ms: 342,
            output: Some("pod deleted".into()),
            error: None,
            trace_id: "abc123".into(),
            span_id: "def456".into(),
        };
        let decoded: ActivityResult = toon_round_trip(&result);
        assert_eq!(decoded, result);
    }

    #[test]
    fn journal_with_all_phases_round_trips() {
        let journal = Journal {
            experiment_title: "Database failover test".into(),
            experiment_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_300_000_000_000,
            duration_ms: 300_000,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![],
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: Some(Estimate {
                expected_outcome: ExpectedOutcome::Recovered,
                expected_recovery_s: Some(15.0),
                expected_degradation: Some(DegradationLevel::Moderate),
                expected_data_loss: Some(false),
                confidence: Some(Confidence::High),
                rationale: None,
                prior_runs: Some(5),
            }),
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
        };
        let decoded: Journal = toon_round_trip(&journal);
        assert_eq!(decoded, journal);
    }

    #[test]
    fn journal_minimal_round_trips() {
        let journal = Journal {
            experiment_title: "Database failover test".into(),
            experiment_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_300_000_000_000,
            duration_ms: 300_000,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![],
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
        };
        let decoded: Journal = toon_round_trip(&journal);
        assert_eq!(decoded, journal);
    }
}
