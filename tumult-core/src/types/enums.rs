//! Simple string-serialized enumerations used throughout the data model.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityType {
    Action,
    Probe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentStatus {
    Completed,
    Deviated,
    Aborted,
    Failed,
    Interrupted,
    /// The run was pulled mid-experiment by an auto-halt guard: a guard probe
    /// breached its safe-condition tolerance while the fault was active, so
    /// the method was cancelled and rollbacks were run. Distinct from
    /// `Aborted` (pre-method hypothesis failed), `Failed` (an action errored),
    /// and `Deviated` (post-method hypothesis failed).
    Halted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Succeeded,
    Failed,
    Timeout,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContainerRuntime {
    Docker,
    Podman,
    Containerd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedOutcome {
    Deviated,
    Recovered,
    Unaffected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradationLevel {
    None,
    Minor,
    Moderate,
    Severe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineMethod {
    Static,
    Percentile,
    MeanStddev,
    Iqr,
    Learned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadTool {
    K6,
    Jmeter,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BaselineSource {
    Live,
    Historical,
    Aqe,
}

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
}
