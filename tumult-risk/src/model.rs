//! Scanner inputs and outputs. The scanner is pure: callers assemble
//! [`Inputs`] from wherever the evidence lives (the lake, a manifests
//! directory, the suppression table) and get a ranked [`Report`] back.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// The resilience disciplines a target can be exercised in, in the order
/// the coverage matrix shows them. Mirrors `TestKind` in the experiment
/// definition and the `experiments.test_kind` lake column. Load and
/// performance are one discipline, `performance`; disaster recovery is
/// part of `robustness`.
pub const KINDS: [&str; 4] = ["chaos", "performance", "robustness", "security"];

/// Nanoseconds in one day.
pub const DAY_NS: i64 = 86_400 * 1_000_000_000;

/// Evidence older than this is stale (the same 30-day window the
/// compliance score uses, so the two never disagree).
pub const STALE_AFTER_NS: i64 = 30 * DAY_NS;

/// Evidence older than this is very stale: the staleness risk escalates.
pub const VERY_STALE_AFTER_NS: i64 = 90 * DAY_NS;

/// The latest evidence of one discipline against one target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KindEvidence {
    pub kind: String,
    pub runs: u64,
    pub last_run_ns: i64,
    pub last_outcome: Option<String>,
}

/// One objective verdict from the latest run of an experiment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Verdict {
    pub nfr_id: String,
    pub kind: String,
    /// `met`, `breached` or `inconclusive`.
    pub state: String,
    /// `blocking`, `major` or `minor`.
    pub criticality: String,
    pub threshold: f64,
    pub observed: Option<f64>,
    pub margin: Option<f64>,
}

/// The latest run of one experiment against a target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExperimentLatest {
    pub name: String,
    /// The discipline it counts towards; `None` for a manual record of a
    /// type the house parameters do not count as a test.
    pub kind: Option<String>,
    pub last_run_ns: i64,
    pub passed: bool,
    pub outcome: Option<String>,
    pub evidence_id: Option<String>,
    #[serde(default)]
    pub verdicts: Vec<Verdict>,
    pub rto_target_s: Option<f64>,
    pub rto_measured_s: Option<f64>,
}

/// Everything known about one target system in one environment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct TargetEvidence {
    pub target: String,
    pub environment: Option<String>,
    /// `critical`, `high`, `medium` or `low` when declared.
    pub criticality: Option<String>,
    #[serde(default)]
    pub kinds: Vec<KindEvidence>,
    #[serde(default)]
    pub experiments: Vec<ExperimentLatest>,
}

/// A service seen in telemetry that other services call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dependency {
    pub service: String,
    pub callers: u64,
    pub calls: u64,
}

/// A registered experiment definition and how often it has run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Definition {
    pub id: String,
    pub name: String,
    pub runs: u64,
}

/// One container of a workload, reduced to what the rules look at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Container {
    pub name: String,
    pub has_limits: bool,
    pub has_readiness_probe: bool,
}

/// A Kubernetes workload (`Deployment`, `StatefulSet`, `DaemonSet`) from a
/// manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workload {
    pub name: String,
    pub namespace: Option<String>,
    pub kind: String,
    /// `None` for kinds without a replica count (`DaemonSet`).
    pub replicas: Option<i64>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub containers: Vec<Container>,
    /// Where the manifest came from (file path), for the evidence list.
    pub source: String,
}

impl Workload {
    /// `namespace/name`, or just the name without a namespace.
    #[must_use]
    pub fn qualified_name(&self) -> String {
        match &self.namespace {
            Some(ns) => format!("{ns}/{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// A `PodDisruptionBudget`: the workloads whose labels match its selector are
/// protected from voluntary disruption.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisruptionBudget {
    pub name: String,
    pub namespace: Option<String>,
    #[serde(default)]
    pub selector: BTreeMap<String, String>,
}

impl DisruptionBudget {
    /// Whether this budget covers the workload: same namespace and every
    /// selector label present on the workload with the same value.
    #[must_use]
    pub fn covers(&self, w: &Workload) -> bool {
        if self.namespace != w.namespace || self.selector.is_empty() {
            return false;
        }
        self.selector
            .iter()
            .all(|(k, v)| w.labels.get(k) == Some(v))
    }
}

/// An operator's decision to silence one risk for a while.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Suppression {
    pub risk_id: String,
    pub reason: String,
    pub actor: String,
    pub at_ns: i64,
    /// `None` never expires.
    pub expires_at_ns: Option<i64>,
}

impl Suppression {
    /// Whether the suppression still applies at `as_of_ns`.
    #[must_use]
    pub fn active_at(&self, as_of_ns: i64) -> bool {
        self.expires_at_ns.is_none_or(|exp| exp > as_of_ns)
    }
}

/// Everything the scanner looks at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Inputs {
    pub as_of_ns: i64,
    #[serde(default)]
    pub targets: Vec<TargetEvidence>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
    #[serde(default)]
    pub definitions: Vec<Definition>,
    #[serde(default)]
    pub workloads: Vec<Workload>,
    #[serde(default)]
    pub budgets: Vec<DisruptionBudget>,
    #[serde(default)]
    pub suppressions: Vec<Suppression>,
}

/// How much a finding matters. Ordered so `Critical > High > Medium > Low`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    /// Base ranking weight.
    #[must_use]
    pub fn base_rank(self) -> u32 {
        match self {
            Self::Critical => 90,
            Self::High => 70,
            Self::Medium => 45,
            Self::Low => 20,
        }
    }

    /// Lowercase name, as serialized.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
        }
    }
}

/// One finding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Risk {
    /// Stable across scans: a hash of rule, target, environment and subject,
    /// so suppressions survive restarts and re-scans.
    pub id: String,
    pub rule: String,
    pub severity: Severity,
    /// 0–100 ranking weight: severity plus target criticality and
    /// rule-specific modifiers. Higher ranks first.
    pub rank: u32,
    pub title: String,
    pub detail: String,
    pub target: Option<String>,
    pub environment: Option<String>,
    /// What the rule looked at (a discipline, an experiment, a workload).
    pub subject: String,
    /// The discipline whose test would retire the risk, when there is one.
    pub recommended_kind: Option<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
    /// Present when an active suppression silences the risk.
    pub suppression: Option<Suppression>,
}

/// Coverage state of one target in one discipline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageStatus {
    Covered,
    Stale,
    Missing,
}

/// One cell of the coverage matrix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoverageCell {
    pub kind: String,
    pub status: CoverageStatus,
    pub runs: u64,
    pub last_run_ns: Option<i64>,
}

/// One row of the coverage matrix.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TargetCoverage {
    pub target: String,
    pub environment: Option<String>,
    pub criticality: Option<String>,
    pub cells: Vec<CoverageCell>,
}

/// Portfolio coverage in one discipline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KindCoverage {
    pub kind: String,
    pub covered: usize,
    pub stale: usize,
    pub missing: usize,
    /// `covered / targets` (stale counts as not covered); 0 with no targets.
    pub ratio: f64,
}

/// The targets × disciplines matrix and its per-discipline rollup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Coverage {
    pub kinds: Vec<String>,
    pub targets: Vec<TargetCoverage>,
    pub portfolio: Vec<KindCoverage>,
}

/// Counts by severity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Summary {
    pub total: usize,
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub suppressed: usize,
}

/// The scanner's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub as_of_ns: i64,
    /// Active risks, highest rank first.
    pub risks: Vec<Risk>,
    /// Risks silenced by an active suppression, highest rank first.
    pub suppressed: Vec<Risk>,
    pub summary: Summary,
    pub coverage: Coverage,
}

/// Weight of a target in portfolio rollups, from its declared criticality.
#[must_use]
pub fn criticality_weight(criticality: Option<&str>) -> f64 {
    match criticality.map(str::to_ascii_lowercase).as_deref() {
        Some("critical") => 3.0,
        Some("high") => 2.0,
        Some("medium") => 1.5,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_orders_from_low_to_critical() {
        assert!(Severity::Critical > Severity::High);
        assert!(Severity::High > Severity::Medium);
        assert!(Severity::Medium > Severity::Low);
        assert_eq!(
            serde_json::to_value(Severity::High).unwrap(),
            serde_json::json!("high")
        );
    }

    #[test]
    fn suppression_expiry_is_exclusive() {
        let s = Suppression {
            risk_id: "r_1".into(),
            reason: "known".into(),
            actor: "op".into(),
            at_ns: 0,
            expires_at_ns: Some(100),
        };
        assert!(s.active_at(99));
        assert!(!s.active_at(100));
        let forever = Suppression {
            expires_at_ns: None,
            ..s
        };
        assert!(forever.active_at(i64::MAX));
    }

    #[test]
    fn budget_covers_workload_by_namespace_and_labels() {
        let w = Workload {
            name: "api".into(),
            namespace: Some("prod".into()),
            kind: "Deployment".into(),
            replicas: Some(3),
            labels: [("app".to_string(), "api".to_string())].into(),
            containers: vec![],
            source: "api.yaml".into(),
        };
        let pdb = DisruptionBudget {
            name: "api-pdb".into(),
            namespace: Some("prod".into()),
            selector: [("app".to_string(), "api".to_string())].into(),
        };
        assert!(pdb.covers(&w));
        let other_ns = DisruptionBudget {
            namespace: Some("staging".into()),
            ..pdb.clone()
        };
        assert!(!other_ns.covers(&w));
        let empty = DisruptionBudget {
            selector: BTreeMap::new(),
            ..pdb
        };
        assert!(!empty.covers(&w));
        assert_eq!(w.qualified_name(), "prod/api");
    }

    #[test]
    fn criticality_weights() {
        let close =
            |c: Option<&str>, want: f64| (criticality_weight(c) - want).abs() < f64::EPSILON;
        assert!(close(Some("critical"), 3.0));
        assert!(close(Some("HIGH"), 2.0));
        assert!(close(Some("medium"), 1.5));
        assert!(close(Some("low"), 1.0));
        assert!(close(None, 1.0));
    }
}
