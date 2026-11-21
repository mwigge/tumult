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
    /// Also read from `timeout`, a status that older versions knew and no run wrote.
    #[serde(alias = "timeout")]
    Failed,
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
    /// `oha` HTTP load generator (JSON summary, one process per window).
    Oha,
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

// ── Resilience disciplines and objectives ─────────────────────

/// The resilience discipline an experiment belongs to. Drives coverage
/// reporting per discipline and which measurements the run is expected to
/// produce (a `dr` run measures RTO/RPO, a `load` run produces load results).
/// Defaults to `chaos` so pre-existing definitions keep their meaning.
/// Non-functional objectives are not a discipline: the `nfr` block of a
/// definition is verified by a run of any kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestKind {
    /// Fault injection against a steady-state hypothesis.
    #[default]
    Chaos,
    /// Load and performance: traffic generation to measure behaviour
    /// under load, and latency/throughput regression against a previous
    /// build. `load` is accepted as another name for it.
    #[serde(alias = "load")]
    Performance,
    /// Soak, saturation and malformed-input profiles.
    Robustness,
    /// Disaster recovery: failover with RTO/RPO targets. Counts towards
    /// the robustness discipline.
    Dr,
    /// Security testing (scans, pentest evidence, misuse cases).
    Security,
}

/// The name a recorded kind is kept under. `load` is another name for
/// `performance` (a manual exercise type, a `resilience.test.kind`
/// attribute from another emitter); every other value is returned as it
/// is.
#[must_use]
pub fn canonical_kind(kind: &str) -> &str {
    if kind == "load" {
        return "performance";
    }
    kind
}

/// The discipline a recorded kind counts towards in coverage and scores.
/// There are four: chaos, performance (load included), robustness and
/// security. Disaster recovery is part of robustness: a `dr` run keeps
/// its kind and its recovery targets, and is robustness evidence.
#[must_use]
pub fn discipline(kind: &str) -> &str {
    match canonical_kind(kind) {
        "dr" => "robustness",
        other => other,
    }
}

/// What a non-functional objective constrains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NfrKind {
    Latency,
    Availability,
    Throughput,
    ErrorRate,
    /// Recovery time objective (seconds).
    Rto,
    /// Recovery point objective (seconds of data loss).
    Rpo,
    Capacity,
    Recovery,
    DataIntegrity,
}

/// Comparison between the observed value and the threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NfrOperator {
    /// Observed must be less than or equal to the threshold.
    Lte,
    /// Observed must be greater than or equal to the threshold.
    Gte,
    /// Observed must equal the threshold (within `f64::EPSILON`).
    Eq,
}

/// How much a breached objective matters to the run verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NfrCriticality {
    /// A breach fails the run outright.
    Blocking,
    /// A breach is reported as a deviation.
    #[default]
    Major,
    /// A breach is informational.
    Minor,
}

/// Outcome of evaluating one objective against a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NfrState {
    Met,
    Breached,
    /// The metric the objective names was not measured in this run.
    Inconclusive,
}

/// Which phase a raw probe sample was taken in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SamplePhase {
    Baseline,
    During,
    Post,
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
            Self::Oha => write!(f, "oha"),
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

impl std::fmt::Display for TestKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Chaos => "chaos",
            Self::Performance => "performance",
            Self::Robustness => "robustness",
            Self::Dr => "dr",
            Self::Security => "security",
        };
        f.write_str(s)
    }
}

impl std::fmt::Display for NfrKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Latency => "latency",
            Self::Availability => "availability",
            Self::Throughput => "throughput",
            Self::ErrorRate => "error_rate",
            Self::Rto => "rto",
            Self::Rpo => "rpo",
            Self::Capacity => "capacity",
            Self::Recovery => "recovery",
            Self::DataIntegrity => "data_integrity",
        };
        f.write_str(s)
    }
}

impl std::fmt::Display for NfrOperator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Lte => "lte",
            Self::Gte => "gte",
            Self::Eq => "eq",
        };
        f.write_str(s)
    }
}

impl std::fmt::Display for NfrCriticality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Blocking => "blocking",
            Self::Major => "major",
            Self::Minor => "minor",
        };
        f.write_str(s)
    }
}

impl std::fmt::Display for NfrState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Met => "met",
            Self::Breached => "breached",
            Self::Inconclusive => "inconclusive",
        };
        f.write_str(s)
    }
}

impl std::fmt::Display for SamplePhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Baseline => "baseline",
            Self::During => "during",
            Self::Post => "post",
        };
        f.write_str(s)
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
        for tool in [LoadTool::K6, LoadTool::Jmeter, LoadTool::Oha] {
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
        assert_eq!(LoadTool::Oha.to_string(), "oha");

        assert_eq!(BaselineSource::Live.to_string(), "live");
        assert_eq!(BaselineSource::Historical.to_string(), "historical");
        assert_eq!(BaselineSource::Aqe.to_string(), "aqe");

        assert_eq!(Trend::Improving.to_string(), "improving");
        assert_eq!(Trend::Stable.to_string(), "stable");
        assert_eq!(Trend::Degrading.to_string(), "degrading");
    }

    /// Non-functional objectives are verified by a run of every discipline;
    /// they are not a discipline of their own.
    #[test]
    fn nfr_is_not_a_test_kind() {
        assert!(serde_json::from_str::<TestKind>("\"nfr\"").is_err());
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

        for kind in [
            TestKind::Chaos,
            TestKind::Performance,
            TestKind::Robustness,
            TestKind::Dr,
            TestKind::Security,
        ] {
            let json = serde_json::to_string(&kind).expect("serialize");
            assert_eq!(json, format!("\"{kind}\""));
        }
        for nfr in [NfrKind::ErrorRate, NfrKind::Rto, NfrKind::DataIntegrity] {
            let json = serde_json::to_string(&nfr).expect("serialize");
            assert_eq!(json, format!("\"{nfr}\""));
        }
        for op in [NfrOperator::Lte, NfrOperator::Gte, NfrOperator::Eq] {
            let json = serde_json::to_string(&op).expect("serialize");
            assert_eq!(json, format!("\"{op}\""));
        }
        let state = NfrState::Inconclusive;
        assert_eq!(
            serde_json::to_string(&state).unwrap(),
            format!("\"{state}\"")
        );
        let phase = SamplePhase::During;
        assert_eq!(
            serde_json::to_string(&phase).unwrap(),
            format!("\"{phase}\"")
        );
    }

    /// Load and performance are one discipline: a definition may say
    /// either, the run is recorded as performance.
    #[test]
    fn load_is_another_name_for_performance() {
        let kind = serde_json::from_str::<TestKind>("\"load\"").expect("load parses");
        assert_eq!(kind, TestKind::Performance);
        assert_eq!(kind.to_string(), "performance");
        assert_eq!(discipline("load"), "performance");
        assert_eq!(discipline("performance"), "performance");
        assert_eq!(canonical_kind("load"), "performance");
    }

    /// Disaster recovery is part of robustness: a dr run keeps its kind
    /// (and its recovery targets) and counts towards robustness.
    #[test]
    fn dr_is_part_of_the_robustness_discipline() {
        assert_eq!(TestKind::Dr.to_string(), "dr");
        assert_eq!(canonical_kind("dr"), "dr");
        assert_eq!(discipline("dr"), "robustness");
        assert_eq!(discipline("robustness"), "robustness");
        assert_eq!(discipline("chaos"), "chaos");
    }

    #[test]
    fn test_kind_defaults_to_chaos_and_rejects_unknown() {
        assert_eq!(TestKind::default(), TestKind::Chaos);
        assert_eq!(
            serde_json::from_str::<TestKind>("\"dr\"").unwrap(),
            TestKind::Dr
        );
        assert!(serde_json::from_str::<TestKind>("\"smoke\"").is_err());
    }
}
