//! Simple string-serialized enumerations used throughout the data model.

use serde::{Deserialize, Serialize};

/// Whether an activity injects a fault or measures the system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityType {
    /// Mutates the system under test (fault injection or rollback).
    Action,
    /// Measures system state without mutating it.
    Probe,
}

/// Terminal status of an experiment run, recorded in the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentStatus {
    /// The run finished and all hypotheses held.
    Completed,
    /// The run finished but a post-method hypothesis failed.
    Deviated,
    /// The run stopped before the method because the pre-method hypothesis failed.
    Aborted,
    /// The run stopped because an action errored.
    Failed,
    /// The run was cancelled mid-execution (e.g. via a cancellation token).
    Interrupted,
    /// The run was pulled mid-experiment by an auto-halt guard: a guard probe
    /// breached its safe-condition tolerance while the fault was active, so
    /// the method was cancelled and rollbacks were run. Distinct from
    /// `Aborted` (pre-method hypothesis failed), `Failed` (an action errored),
    /// and `Deviated` (post-method hypothesis failed).
    Halted,
}

/// Outcome status of a single activity execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Succeeded,
    Failed,
    /// The activity exceeded its timeout and was terminated.
    Timeout,
    /// The activity was not executed.
    Skipped,
}

/// Container runtime used to target or inspect containers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerRuntime {
    Docker,
    Podman,
    Containerd,
}

/// The outcome an operator predicts in the Phase 0 estimate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedOutcome {
    /// The system is expected to deviate from steady state and not recover cleanly.
    Deviated,
    /// The system is expected to deviate, then recover.
    Recovered,
    /// The system is expected to be unaffected by the fault.
    Unaffected,
}

/// Severity of expected or observed service degradation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradationLevel {
    None,
    Minor,
    Moderate,
    Severe,
}

/// Operator confidence in a Phase 0 estimate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

/// Statistical method used to derive baseline tolerance bounds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineMethod {
    /// Operator-provided fixed bounds.
    Static,
    /// Bounds derived from sample percentiles.
    Percentile,
    /// Bounds at `mean ± sigma * stddev`.
    MeanStddev,
    /// Bounds derived from the interquartile range.
    Iqr,
    /// Bounds learned from historical data rather than the live window.
    Learned,
}

/// Load generator tool supported by the runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadTool {
    K6,
    Jmeter,
}

/// Where baseline data came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineSource {
    /// Sampled live before fault injection.
    Live,
    /// Loaded from a previous run's journal or baseline.
    Historical,
    /// Provided by the AQE subsystem.
    Aqe,
}

/// Direction of change in resilience metrics across runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trend {
    Improving,
    Stable,
    Degrading,
}

// ── Display impls ─────────────────────────────────────────────

impl std::fmt::Display for ActivityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Action => write!(f, "action"),
            Self::Probe => write!(f, "probe"),
        }
    }
}

impl std::fmt::Display for ExperimentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completed => write!(f, "completed"),
            Self::Deviated => write!(f, "deviated"),
            Self::Aborted => write!(f, "aborted"),
            Self::Failed => write!(f, "failed"),
            Self::Interrupted => write!(f, "interrupted"),
            Self::Halted => write!(f, "halted"),
        }
    }
}

impl std::fmt::Display for ActivityStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Succeeded => write!(f, "succeeded"),
            Self::Failed => write!(f, "failed"),
            Self::Timeout => write!(f, "timeout"),
            Self::Skipped => write!(f, "skipped"),
        }
    }
}

impl std::fmt::Display for ContainerRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Docker => write!(f, "docker"),
            Self::Podman => write!(f, "podman"),
            Self::Containerd => write!(f, "containerd"),
        }
    }
}

impl std::fmt::Display for ExpectedOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Deviated => write!(f, "deviated"),
            Self::Recovered => write!(f, "recovered"),
            Self::Unaffected => write!(f, "unaffected"),
        }
    }
}

impl std::fmt::Display for DegradationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Minor => write!(f, "minor"),
            Self::Moderate => write!(f, "moderate"),
            Self::Severe => write!(f, "severe"),
        }
    }
}

impl std::fmt::Display for Confidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Low => write!(f, "low"),
            Self::Medium => write!(f, "medium"),
            Self::High => write!(f, "high"),
        }
    }
}

impl std::fmt::Display for BaselineMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Static => write!(f, "static"),
            Self::Percentile => write!(f, "percentile"),
            Self::MeanStddev => write!(f, "mean_stddev"),
            Self::Iqr => write!(f, "iqr"),
            Self::Learned => write!(f, "learned"),
        }
    }
}

impl std::fmt::Display for LoadTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::K6 => write!(f, "k6"),
            Self::Jmeter => write!(f, "jmeter"),
        }
    }
}

impl std::fmt::Display for BaselineSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Live => write!(f, "live"),
            Self::Historical => write!(f, "historical"),
            Self::Aqe => write!(f, "aqe"),
        }
    }
}

impl std::fmt::Display for Trend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Improving => write!(f, "improving"),
            Self::Stable => write!(f, "stable"),
            Self::Degrading => write!(f, "degrading"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::types::test_support::toon_round_trip;
    use crate::types::*;

    #[test]
    fn activity_type_action_round_trips() {
        let at = ActivityType::Action;
        let decoded: ActivityType = toon_round_trip(&at);
        assert_eq!(decoded, ActivityType::Action);
    }

    #[test]
    fn activity_type_probe_round_trips() {
        let at = ActivityType::Probe;
        let decoded: ActivityType = toon_round_trip(&at);
        assert_eq!(decoded, ActivityType::Probe);
    }

    #[test]
    fn experiment_status_all_variants_round_trip() {
        for status in [
            ExperimentStatus::Completed,
            ExperimentStatus::Deviated,
            ExperimentStatus::Aborted,
            ExperimentStatus::Failed,
            ExperimentStatus::Interrupted,
            ExperimentStatus::Halted,
        ] {
            let decoded: ExperimentStatus = toon_round_trip(&status);
            assert_eq!(decoded, status);
        }
    }

    #[test]
    fn activity_status_all_variants_round_trip() {
        for status in [
            ActivityStatus::Succeeded,
            ActivityStatus::Failed,
            ActivityStatus::Timeout,
            ActivityStatus::Skipped,
        ] {
            let decoded: ActivityStatus = toon_round_trip(&status);
            assert_eq!(decoded, status);
        }
    }

    #[test]
    fn container_runtime_round_trips() {
        for rt in [
            ContainerRuntime::Docker,
            ContainerRuntime::Podman,
            ContainerRuntime::Containerd,
        ] {
            let decoded: ContainerRuntime = toon_round_trip(&rt);
            assert_eq!(decoded, rt);
        }
    }

    #[test]
    fn trend_all_variants_round_trip() {
        for trend in [Trend::Improving, Trend::Stable, Trend::Degrading] {
            let decoded: Trend = toon_round_trip(&trend);
            assert_eq!(decoded, trend);
        }
    }

    #[test]
    fn baseline_source_all_variants_round_trip() {
        for source in [
            BaselineSource::Live,
            BaselineSource::Historical,
            BaselineSource::Aqe,
        ] {
            let decoded: BaselineSource = toon_round_trip(&source);
            assert_eq!(decoded, source);
        }
    }

    #[test]
    fn expected_outcome_all_variants_round_trip() {
        for outcome in [
            ExpectedOutcome::Deviated,
            ExpectedOutcome::Recovered,
            ExpectedOutcome::Unaffected,
        ] {
            let decoded: ExpectedOutcome = toon_round_trip(&outcome);
            assert_eq!(decoded, outcome);
        }
    }

    #[test]
    fn degradation_level_all_variants_round_trip() {
        for level in [
            DegradationLevel::None,
            DegradationLevel::Minor,
            DegradationLevel::Moderate,
            DegradationLevel::Severe,
        ] {
            let decoded: DegradationLevel = toon_round_trip(&level);
            assert_eq!(decoded, level);
        }
    }

    #[test]
    fn confidence_all_variants_round_trip() {
        for confidence in [Confidence::Low, Confidence::Medium, Confidence::High] {
            let decoded: Confidence = toon_round_trip(&confidence);
            assert_eq!(decoded, confidence);
        }
    }

    #[test]
    fn baseline_method_all_variants_round_trip() {
        for method in [
            BaselineMethod::Static,
            BaselineMethod::Percentile,
            BaselineMethod::MeanStddev,
            BaselineMethod::Iqr,
            BaselineMethod::Learned,
        ] {
            let decoded: BaselineMethod = toon_round_trip(&method);
            assert_eq!(decoded, method);
        }
    }

    #[test]
    fn load_tool_all_variants_round_trip() {
        for tool in [LoadTool::K6, LoadTool::Jmeter] {
            let decoded: LoadTool = toon_round_trip(&tool);
            assert_eq!(decoded, tool);
        }
    }

    #[test]
    fn display_renders_the_snake_case_wire_name() {
        assert_eq!(ActivityType::Action.to_string(), "action");
        assert_eq!(ActivityType::Probe.to_string(), "probe");

        assert_eq!(ExperimentStatus::Completed.to_string(), "completed");
        assert_eq!(ExperimentStatus::Deviated.to_string(), "deviated");
        assert_eq!(ExperimentStatus::Aborted.to_string(), "aborted");
        assert_eq!(ExperimentStatus::Failed.to_string(), "failed");
        assert_eq!(ExperimentStatus::Interrupted.to_string(), "interrupted");
        assert_eq!(ExperimentStatus::Halted.to_string(), "halted");

        assert_eq!(ActivityStatus::Succeeded.to_string(), "succeeded");
        assert_eq!(ActivityStatus::Failed.to_string(), "failed");
        assert_eq!(ActivityStatus::Timeout.to_string(), "timeout");
        assert_eq!(ActivityStatus::Skipped.to_string(), "skipped");

        assert_eq!(ContainerRuntime::Docker.to_string(), "docker");
        assert_eq!(ContainerRuntime::Podman.to_string(), "podman");
        assert_eq!(ContainerRuntime::Containerd.to_string(), "containerd");

        assert_eq!(ExpectedOutcome::Deviated.to_string(), "deviated");
        assert_eq!(ExpectedOutcome::Recovered.to_string(), "recovered");
        assert_eq!(ExpectedOutcome::Unaffected.to_string(), "unaffected");

        assert_eq!(DegradationLevel::None.to_string(), "none");
        assert_eq!(DegradationLevel::Minor.to_string(), "minor");
        assert_eq!(DegradationLevel::Moderate.to_string(), "moderate");
        assert_eq!(DegradationLevel::Severe.to_string(), "severe");

        assert_eq!(Confidence::Low.to_string(), "low");
        assert_eq!(Confidence::Medium.to_string(), "medium");
        assert_eq!(Confidence::High.to_string(), "high");

        assert_eq!(BaselineMethod::Static.to_string(), "static");
        assert_eq!(BaselineMethod::Percentile.to_string(), "percentile");
        assert_eq!(BaselineMethod::MeanStddev.to_string(), "mean_stddev");
        assert_eq!(BaselineMethod::Iqr.to_string(), "iqr");
        assert_eq!(BaselineMethod::Learned.to_string(), "learned");

        assert_eq!(LoadTool::K6.to_string(), "k6");
        assert_eq!(LoadTool::Jmeter.to_string(), "jmeter");

        assert_eq!(BaselineSource::Live.to_string(), "live");
        assert_eq!(BaselineSource::Historical.to_string(), "historical");
        assert_eq!(BaselineSource::Aqe.to_string(), "aqe");

        assert_eq!(Trend::Improving.to_string(), "improving");
        assert_eq!(Trend::Stable.to_string(), "stable");
        assert_eq!(Trend::Degrading.to_string(), "degrading");
    }

    #[test]
    fn display_matches_the_serde_serialization() {
        // The journal and the CLI rely on Display and serde agreeing; guard
        // the two from drifting apart.
        let status = ExperimentStatus::Halted;
        let json = serde_json::to_string(&status).expect("serialize");
        assert_eq!(json, format!("\"{status}\""));

        let method = BaselineMethod::MeanStddev;
        let json = serde_json::to_string(&method).expect("serialize");
        assert_eq!(json, format!("\"{method}\""));
    }
}
