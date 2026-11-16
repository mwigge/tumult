//! Experiment journal output types.

use serde::{Deserialize, Serialize};

use super::definition::{Estimate, Experiment, RegulatoryMapping};
use super::enums::{ActivityStatus, ActivityType, ExperimentStatus, TestKind};
use super::ids::{SpanId, TraceId};
use super::results::{
    AnalysisResult, BaselineResult, DuringResult, LoadResult, ObjectiveOutcome, PostResult,
    ProbeSample,
};

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

/// Where the version recorded on a run came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionSource {
    /// The definition's `target.version`, or `--target-version`.
    Declared,
    /// Resolved once at run end from the target's own telemetry inside
    /// the run's window, in the run's environment.
    Observed,
    /// The telemetry inside the run's window named more than one version
    /// and none served [`OBSERVED_VERSION_MIN_SHARE`] of the requests: the
    /// run measured a mix, and belongs to no single version.
    Mixed,
}

/// The share of a run's requests one version must have served for the
/// run to be recorded as observed on that version.
pub const OBSERVED_VERSION_MIN_SHARE: f64 = 0.95;

/// The fewest requests the target's telemetry must have counted inside a
/// run's window, as a share of the requests the run's load sent, for the
/// run to record a version from it. Low, because telemetry may be
/// sampled; not zero, because one request of an old build in a window
/// whose other telemetry has not arrived says nothing of what served the
/// load.
pub const OBSERVED_MIN_LOAD_SHARE: f64 = 0.01;

/// How many of the requests inside a run's window one version served.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionShare {
    pub version: String,
    pub requests: u64,
}

impl std::fmt::Display for VersionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Declared => "declared",
            Self::Observed => "observed",
            Self::Mixed => "mixed",
        })
    }
}

/// The version of the target a run measured, frozen in the journal when
/// the run ended so that later telemetry cannot move it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetVersion {
    /// The version; for a [`VersionSource::Mixed`] run the one that
    /// served most, which the run is not attributed to.
    pub version: String,
    pub version_source: VersionSource,
    /// What the telemetry said when it named more than one version, most
    /// served first. Empty for a declared version and for a window one
    /// version served alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shares: Vec<VersionShare>,
}

impl TargetVersion {
    /// What a run records from the versions its target's telemetry named
    /// inside its window: `None` when it named none.
    #[must_use]
    pub fn observed(seen: Vec<VersionShare>) -> Option<Self> {
        let mut seen: Vec<VersionShare> = seen
            .into_iter()
            .map(|share| VersionShare {
                version: share.version.trim().to_string(),
                requests: share.requests,
            })
            .filter(|share| !share.version.is_empty() && share.requests > 0)
            .collect();
        seen.sort_by(|a, b| {
            b.requests
                .cmp(&a.requests)
                .then_with(|| a.version.cmp(&b.version))
        });
        let most = seen.first()?;
        let all: u64 = seen.iter().map(|share| share.requests).sum();
        // Request counts are far below 2^53.
        #[allow(clippy::cast_precision_loss)]
        let version_source = if most.requests as f64 >= OBSERVED_VERSION_MIN_SHARE * all as f64 {
            VersionSource::Observed
        } else {
            VersionSource::Mixed
        };
        Some(Self {
            version: most.version.clone(),
            version_source,
            shares: if seen.len() > 1 { seen } else { Vec::new() },
        })
    }

    /// The requests of `seen` that name a version.
    #[must_use]
    pub fn versioned_requests(seen: &[VersionShare]) -> u64 {
        seen.iter()
            .filter(|share| !share.version.trim().is_empty())
            .map(|share| share.requests)
            .sum()
    }

    /// [`TargetVersion::observed`] for a run whose load sent `sent`
    /// requests: `None` also when the telemetry counted fewer than
    /// [`OBSERVED_MIN_LOAD_SHARE`] of them. A load that counted nothing
    /// asks for nothing.
    #[must_use]
    pub fn observed_under_load(seen: Vec<VersionShare>, sent: u64) -> Option<Self> {
        // Request counts are far below 2^53.
        #[allow(clippy::cast_precision_loss)]
        let enough =
            Self::versioned_requests(&seen) as f64 >= OBSERVED_MIN_LOAD_SHARE * sent as f64;
        if !enough {
            return None;
        }
        Self::observed(seen)
    }

    /// The share of the window's requests the recorded version served;
    /// `None` when no shares were recorded.
    #[must_use]
    pub fn share(&self) -> Option<f64> {
        let own = self
            .shares
            .iter()
            .find(|share| share.version == self.version)?;
        let all: u64 = self.shares.iter().map(|share| share.requests).sum();
        // Request counts are far below 2^53.
        #[allow(clippy::cast_precision_loss)]
        Some(own.requests as f64 / all as f64)
    }

    /// The version the definition declares, if it declares one.
    #[must_use]
    pub fn declared(experiment: &Experiment) -> Option<Self> {
        let version = experiment.target.as_ref()?.declared_version()?;
        Some(Self {
            version: version.to_string(),
            version_source: VersionSource::Declared,
            shares: Vec::new(),
        })
    }
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
    /// Resilience discipline, copied from the definition.
    #[serde(default)]
    pub kind: TestKind,
    /// Recovery targets, measurements and objective verdicts; `None` when
    /// the definition declared no objectives or targets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objectives: Option<ObjectiveOutcome>,
    /// Raw probe samples from the during and post phases, in collection
    /// order. Aggregates live in `during_result` / `post_result`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub probe_samples: Vec<ProbeSample>,
    /// The version of the target this run measured and how it is known;
    /// `None` when it was neither declared nor observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_version: Option<TargetVersion>,
}

impl Journal {
    /// Build a `Journal` skeleton for `experiment`: fills in the fields
    /// derived from the experiment definition (`experiment_title`,
    /// `estimate`, `regulatory`, a declared target version) plus the given identity/status/timing, and
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
            kind: experiment.kind,
            objectives: None,
            probe_samples: vec![],
            target_version: TargetVersion::declared(experiment),
        }
    }

    /// Whether the steady state held, as far as the run judged it: the
    /// verdict after the method when the run got that far, and `false` when
    /// the hypothesis failed before the method. `None` when the definition
    /// has no hypothesis, or when it held before the method and the run
    /// ended without judging it again.
    #[must_use]
    pub fn hypothesis_met(&self) -> Option<bool> {
        if let Some(after) = &self.steady_state_after {
            return Some(after.met);
        }
        let before = self.steady_state_before.as_ref()?;
        (!before.met).then_some(false)
    }

    /// The measured recovery time: seconds from the end of the method until
    /// every hypothesis probe was back within tolerance. `None` when the
    /// run did not see the target recover.
    #[must_use]
    pub fn recovery_time_s(&self) -> Option<f64> {
        self.post_result.as_ref()?.mttr_s
    }
}

#[cfg(test)]
mod tests {
    use crate::types::test_support::toon_round_trip;
    use crate::types::*;

    fn share(version: &str, requests: u64) -> VersionShare {
        VersionShare {
            version: version.into(),
            requests,
        }
    }

    #[test]
    fn a_load_s_version_needs_a_share_of_the_requests_it_sent() {
        let seen = |requests| vec![share("1.4.0", requests), share(" ", 500)];
        // One request in 4000 says nothing of what served the other 3999.
        assert_eq!(TargetVersion::observed_under_load(seen(1), 4000), None);
        assert_eq!(TargetVersion::observed_under_load(seen(39), 4000), None);
        // 1 % is enough: telemetry may be sampled.
        let enough = TargetVersion::observed_under_load(seen(40), 4000).unwrap();
        assert_eq!(enough.version_source, VersionSource::Observed);
        assert_eq!(
            TargetVersion::observed_under_load(seen(1), 0),
            TargetVersion::observed(seen(1))
        );
        assert_eq!(TargetVersion::observed_under_load(vec![], 0), None);
    }

    #[test]
    fn a_window_is_observed_on_a_version_only_when_it_served_nearly_all_of_it() {
        assert_eq!(TargetVersion::observed(vec![]), None);
        // Alone: observed, nothing more to say.
        let alone = TargetVersion::observed(vec![share("1.5.1", 40)]).unwrap();
        assert_eq!(
            (
                alone.version.as_str(),
                alone.version_source,
                alone.shares.len()
            ),
            ("1.5.1", VersionSource::Observed, 0)
        );
        assert_eq!(alone.share(), None);
        // 97 %: observed, and the stray is on record. The order given
        // does not matter, blanks and silent versions are left out.
        let nearly = TargetVersion::observed(vec![
            share("1.5.0", 3),
            share(" ", 50),
            share("1.4.0", 0),
            share(" 1.5.1 ", 97),
        ])
        .unwrap();
        assert_eq!(nearly.version, "1.5.1");
        assert_eq!(nearly.version_source, VersionSource::Observed);
        assert_eq!(nearly.shares, vec![share("1.5.1", 97), share("1.5.0", 3)]);
        assert!((nearly.share().unwrap() - 0.97).abs() < 1e-12);
        // Exactly the bound is enough; a request under it is not.
        let at = TargetVersion::observed(vec![share("b", 95), share("a", 5)]).unwrap();
        assert_eq!(at.version_source, VersionSource::Observed);
        let mixed = TargetVersion::observed(vec![share("b", 94), share("a", 6)]).unwrap();
        assert_eq!(mixed.version_source, VersionSource::Mixed);
        assert_eq!(mixed.version, "b");
        assert_eq!(mixed.shares, vec![share("b", 94), share("a", 6)]);
        // A tie is broken by the text, so the record is the same each time.
        let tie = TargetVersion::observed(vec![share("b", 5), share("a", 5)]).unwrap();
        assert_eq!(
            (tie.version.as_str(), tie.version_source),
            ("a", VersionSource::Mixed)
        );
        assert_eq!(VersionSource::Mixed.to_string(), "mixed");
    }

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
            target_version: None,
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
            kind: TestKind::default(),
            objectives: None,
            probe_samples: vec![],
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
    fn the_recorded_target_version_round_trips_with_its_source() {
        let experiment = Experiment {
            title: "t".into(),
            target: Some(ExperimentTarget {
                system: "checkout".into(),
                technology: None,
                environment: None,
                criticality: None,
                namespace: None,
                version: Some(" 1.5.0 ".into()),
            }),
            ..Experiment::default()
        };
        let mut journal =
            Journal::for_experiment(&experiment, "id".into(), ExperimentStatus::Completed, 1);
        assert_eq!(
            journal.target_version,
            Some(TargetVersion {
                version: "1.5.0".into(),
                version_source: VersionSource::Declared,
                shares: Vec::new(),
            }),
            "a declared version is on every journal built for the experiment"
        );
        let encoded = toon_format::encode_default(&journal).expect("encode");
        assert!(encoded.contains("version_source: declared"), "{encoded}");
        assert_eq!(toon_round_trip(&journal), journal);

        journal.target_version = Some(TargetVersion {
            version: "1.5.1".into(),
            version_source: VersionSource::Observed,
            shares: Vec::new(),
        });
        let encoded = toon_format::encode_default(&journal).expect("encode");
        assert!(encoded.contains("version_source: observed"), "{encoded}");
        assert_eq!(toon_round_trip(&journal), journal);

        journal.target_version =
            TargetVersion::observed(vec![share("1.5.1", 60), share("1.5.0", 40)]);
        let encoded = toon_format::encode_default(&journal).expect("encode");
        assert!(encoded.contains("version_source: mixed"), "{encoded}");
        assert_eq!(toon_round_trip(&journal), journal);

        // A journal written before the field existed still reads.
        journal.target_version = None;
        let encoded = toon_format::encode_default(&journal).expect("encode");
        assert!(!encoded.contains("target_version"), "{encoded}");
        assert_eq!(toon_round_trip(&journal), journal);
    }

    #[test]
    fn journal_minimal_round_trips() {
        let journal = Journal {
            target_version: None,
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
            kind: TestKind::default(),
            objectives: None,
            probe_samples: vec![],
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

    fn judged(met: bool) -> HypothesisResult {
        HypothesisResult {
            title: "steady".into(),
            met,
            probe_results: vec![],
        }
    }

    /// A journal whose hypothesis was judged before and after the method
    /// with these verdicts; `None` where it was not judged.
    fn journal_judged(before: Option<bool>, after: Option<bool>) -> Journal {
        Journal {
            steady_state_before: before.map(judged),
            steady_state_after: after.map(judged),
            ..Journal::for_experiment(
                &Experiment::default(),
                "id-1".into(),
                ExperimentStatus::Completed,
                0,
            )
        }
    }

    #[test]
    fn the_verdict_after_the_method_is_the_verdict_on_the_hypothesis() {
        let held = journal_judged(Some(true), Some(true));
        let broke = journal_judged(Some(true), Some(false));

        assert_eq!(held.hypothesis_met(), Some(true));
        assert_eq!(broke.hypothesis_met(), Some(false));
    }

    #[test]
    fn a_hypothesis_that_failed_before_the_method_is_not_met() {
        assert_eq!(
            journal_judged(Some(false), None).hypothesis_met(),
            Some(false)
        );
    }

    #[test]
    fn a_hypothesis_that_only_held_before_the_method_has_no_verdict() {
        assert_eq!(journal_judged(Some(true), None).hypothesis_met(), None);
        assert_eq!(journal_judged(None, None).hypothesis_met(), None);
    }
}
