//! Experiment journal output types.

use serde::{Deserialize, Serialize};

use super::definition::{Estimate, Experiment, RegulatoryMapping};
use super::enums::{ActivityStatus, ActivityType, ExperimentStatus};
use super::ids::{SpanId, TraceId};
use super::results::{AnalysisResult, BaselineResult, DuringResult, LoadResult, PostResult};

/// Recorded outcome of a single executed activity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityResult {
    /// Name of the activity as declared in the experiment.
    pub name: String,
    pub activity_type: ActivityType,
    pub status: ActivityStatus,
    /// Epoch-nanosecond timestamp at which execution started.
    pub started_at_ns: i64,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
    /// Provider output captured on success, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Failure reason captured on error, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Trace the execution span belongs to (empty when uninstrumented).
    pub trace_id: TraceId,
    /// Span recorded for this execution (empty when uninstrumented).
    pub span_id: SpanId,
}

/// Outcome of evaluating a steady-state hypothesis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HypothesisResult {
    /// Title of the hypothesis as declared in the experiment.
    pub title: String,
    /// Whether every probe in `probe_results` succeeded.
    pub met: bool,
    pub probe_results: Vec<ActivityResult>,
}

/// Record of an auto-halt: which guard breached, what it observed, and the
/// timing of the halt. Present on a [`Journal`] only when
/// `status == ExperimentStatus::Halted`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HaltRecord {
    /// Name of the guard whose safe-condition tolerance was breached.
    pub guard_name: String,
    /// The observed probe output that breached the guard (raw provider text),
    /// omitted when the breaching probe produced no output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<String>,
    /// Human-readable description of the safe condition that was violated
    /// (e.g. `range [0, 0.05]`).
    pub safe_condition: String,
    /// Number of consecutive breaches observed before halting (>= the guard's
    /// `min_breaches`).
    pub breach_count: u32,
    /// Epoch-nanosecond timestamp of the breaching sample.
    pub breached_at_ns: i64,
    /// Elapsed time from method start to the halt signal, in milliseconds.
    pub time_to_halt_ms: u64,
    /// Time spent running rollbacks after the halt, in milliseconds.
    pub rollback_ms: u64,
}

/// Blast-radius metadata surfaced in the journal: the author's note, the
/// enforced concurrent-fault cap, and the peak concurrency actually observed
/// during method execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlastRadiusRecord {
    /// Free-form note from the experiment's `blast_radius` field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The enforced cap on concurrent background faults, if one was set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_faults: Option<u32>,
    /// Peak number of background faults observed running concurrently.
    pub peak_concurrent_faults: u32,
}

/// Complete record of an experiment run, serialized to TOON as the run's
/// journal. `Option` result fields are `None` when the corresponding phase
/// did not run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    pub experiment_title: String,
    pub experiment_id: String,
    pub status: ExperimentStatus,
    /// Epoch-nanosecond timestamp at which the run started.
    pub started_at_ns: i64,
    /// Epoch-nanosecond timestamp at which the run ended.
    pub ended_at_ns: i64,
    /// Total run duration in milliseconds.
    pub duration_ms: u64,
    /// Pre-method hypothesis evaluation, if a hypothesis was declared.
    pub steady_state_before: Option<HypothesisResult>,
    /// Post-method hypothesis evaluation, if a hypothesis was declared.
    pub steady_state_after: Option<HypothesisResult>,
    pub method_results: Vec<ActivityResult>,
    pub rollback_results: Vec<ActivityResult>,
    /// Number of rollback activities that failed during execution.
    #[serde(default)]
    pub rollback_failures: u32,
    /// Phase 0 prediction, copied from the experiment definition.
    pub estimate: Option<Estimate>,
    pub baseline_result: Option<BaselineResult>,
    pub during_result: Option<DuringResult>,
    pub post_result: Option<PostResult>,
    pub load_result: Option<LoadResult>,
    pub analysis: Option<AnalysisResult>,
    pub regulatory: Option<RegulatoryMapping>,
    /// Auto-halt record: set iff `status == ExperimentStatus::Halted`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub halt: Option<HaltRecord>,
    /// Blast-radius metadata: present when the experiment declared a
    /// `blast_radius` note and/or `max_concurrent_faults`, or when any
    /// background faults ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blast_radius: Option<BlastRadiusRecord>,
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
            halt: None,
            blast_radius: None,
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
            halt: None,
            blast_radius: None,
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
            halt: None,
            blast_radius: None,
        };
        let decoded: Journal = toon_round_trip(&journal);
        assert_eq!(decoded, journal);
    }

    #[test]
    fn halt_record_round_trips() {
        let halt = HaltRecord {
            guard_name: "error-rate-slo".into(),
            observed: Some("0.42".into()),
            safe_condition: "range [0, 0.05]".into(),
            breach_count: 3,
            breached_at_ns: 1_774_980_136_000_000_000,
            time_to_halt_ms: 1_240,
            rollback_ms: 87,
        };
        let decoded: HaltRecord = toon_round_trip(&halt);
        assert_eq!(decoded, halt);
    }

    #[test]
    fn blast_radius_record_round_trips() {
        let br = BlastRadiusRecord {
            note: Some("payments namespace only".into()),
            max_concurrent_faults: Some(2),
            peak_concurrent_faults: 2,
        };
        let decoded: BlastRadiusRecord = toon_round_trip(&br);
        assert_eq!(decoded, br);
    }

    #[test]
    fn halted_journal_round_trips() {
        let journal = Journal {
            status: ExperimentStatus::Halted,
            halt: Some(HaltRecord {
                guard_name: "g".into(),
                observed: None,
                safe_condition: "range [0, 1]".into(),
                breach_count: 1,
                breached_at_ns: 42,
                time_to_halt_ms: 10,
                rollback_ms: 5,
            }),
            blast_radius: Some(BlastRadiusRecord {
                note: None,
                max_concurrent_faults: Some(1),
                peak_concurrent_faults: 1,
            }),
            ..Journal::for_experiment(
                &Experiment::default(),
                "id-1".into(),
                ExperimentStatus::Halted,
                0,
            )
        };
        let decoded: Journal = toon_round_trip(&journal);
        assert_eq!(decoded, journal);
    }
}
