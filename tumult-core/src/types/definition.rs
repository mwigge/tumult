//! Experiment authoring types: the top-level `Experiment` and its sections.

use std::collections::HashMap;
use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::enums::{
    ActivityType, BaselineMethod, Confidence, DegradationLevel, ExpectedOutcome, LoadTool,
    NfrCriticality, NfrKind, NfrOperator, TestKind,
};
use super::target::{ConfigValue, Provider, SecretValue, Tolerance};

// ── Activity ───────────────────────────────────────────────────

/// A single experiment step: an action (fault injection) or a probe
/// (measurement), executed through a provider.
///
/// A key that is not listed here is an error. A dropped key would change
/// what the step does without a word, for example where it runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Activity {
    /// Human-readable step name, surfaced in the journal.
    pub name: String,
    pub activity_type: ActivityType,
    pub provider: Provider,
    /// Expected-output check. `None` (the default) means success is judged
    /// by the provider's exit status alone.
    #[serde(default)]
    pub tolerance: Option<Tolerance>,
    /// Pause in seconds before the activity runs.
    #[serde(default)]
    pub pause_before_s: Option<f64>,
    /// Pause in seconds after the activity completes.
    #[serde(default)]
    pub pause_after_s: Option<f64>,
    /// Run concurrently with the method instead of sequentially.
    #[serde(default)]
    pub background: bool,
    /// Optional label selector for targeting specific pods or containers by labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_selector: Option<HashMap<String, String>>,
}

impl Default for Activity {
    fn default() -> Self {
        Self {
            name: String::new(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "echo".into(),
                arguments: vec![],
                env: HashMap::new(),
                timeout_s: None,
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        }
    }
}

// ── Hypothesis ─────────────────────────────────────────────────

/// Steady-state hypothesis: the probes that define "healthy" for the system
/// under test. Evaluated before and after the method.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hypothesis {
    /// Human-readable statement of the expected steady state.
    pub title: String,
    /// Probes evaluated together; the hypothesis is met when all succeed.
    pub probes: Vec<Activity>,
}

// ── Guard (auto-halt condition) ────────────────────────────────

/// A continuously-evaluated safety guard. While the method (fault window) is
/// active, the runner samples `probe` on the sampling interval. The probe's
/// `tolerance` describes the **safe** condition; a *breach* (tolerance NOT
/// met) means the blast radius has grown unsafe, and the runner halts the
/// experiment: the method is cancelled, rollbacks run, and the journal status
/// becomes [`ExperimentStatus::Halted`](super::enums::ExperimentStatus).
///
/// A guard is just an ordinary probe plus a debounce, so any SLO check that
/// can be expressed as a process probe (e.g. `curl` against a Prometheus API)
/// or a native probe with a tolerance can be a guard — no new provider is
/// needed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Guard {
    pub name: String,
    /// The probe evaluated on the sampling interval. Its `tolerance` is the
    /// **safe range/condition**; breach ⇒ halt.
    pub probe: Activity,
    /// Number of consecutive breaches required before halting, to debounce
    /// transient spikes. Defaults to 1 (halt on the first breach).
    #[serde(default = "default_min_breaches")]
    pub min_breaches: u32,
}

fn default_min_breaches() -> u32 {
    1
}

// ── Control ────────────────────────────────────────────────────

/// A lifecycle control binding: names a control and the provider that
/// implements its hooks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    /// Control identifier matched at lifecycle points (e.g. `logging`).
    pub name: String,
    /// Provider invoked when the control's lifecycle points fire.
    pub provider: Provider,
}

// ── Estimate (Phase 0) ────────────────────────────────────────

/// Phase 0 estimate: the operator's predicted outcome, recorded before the
/// run and compared against actuals during analysis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Estimate {
    /// Predicted outcome of the experiment.
    pub expected_outcome: ExpectedOutcome,
    /// Predicted recovery time in seconds.
    pub expected_recovery_s: Option<f64>,
    /// Predicted severity of service degradation during the fault.
    pub expected_degradation: Option<DegradationLevel>,
    /// Whether the operator expects data loss.
    pub expected_data_loss: Option<bool>,
    /// Operator's confidence in the prediction.
    pub confidence: Option<Confidence>,
    /// Free-form reasoning behind the prediction.
    pub rationale: Option<String>,
    /// Number of prior runs informing the prediction.
    pub prior_runs: Option<u32>,
}

// ── Baseline Config (Phase 1) ──────────────────────────────────

/// Phase 1 baseline configuration: how steady-state metrics are captured
/// before fault injection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineConfig {
    /// Total baseline capture window in seconds.
    pub duration_s: f64,
    /// Warmup period in seconds at the start of the window.
    pub warmup_s: Option<f64>,
    /// Interval in seconds between probe samples.
    pub interval_s: f64,
    /// Statistical method used to derive tolerance bounds.
    pub method: BaselineMethod,
    /// Standard-deviation multiplier, used by the `mean_stddev` method.
    pub sigma: Option<f64>,
    /// Confidence level (0.0-1.0), used by statistical methods that take one.
    pub confidence: Option<f64>,
}

// ── Recovery window (Phase 3) ──────────────────────────────────

/// How the post phase watches for recovery after the method: it reads the
/// steady-state probes every `interval_s` until they pass or `window_s`
/// has passed. An experiment without the block watches as
/// [`RecoveryConfig::default`] says: 30 seconds, read every second.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryConfig {
    /// Seconds the post phase watches, at most
    /// [`RecoveryConfig::MAX_WINDOW_S`].
    #[serde(default = "default_recovery_window_s")]
    pub window_s: f64,
    /// Seconds between two reads of the probes, at most `window_s`.
    #[serde(default = "default_recovery_interval_s")]
    pub interval_s: f64,
}

impl RecoveryConfig {
    /// The longest window a run may watch: one hour. A run must not wait
    /// for ever on a target that does not come back.
    pub const MAX_WINDOW_S: f64 = 3600.0;

    /// Why the post phase cannot watch this window, or `None` when it can:
    /// both numbers are above 0, the window is at most an hour, and the
    /// interval is at most the window. Validation and the runner both ask.
    #[must_use]
    pub fn invalid_reason(&self) -> Option<String> {
        if self.window_s.is_nan() || self.window_s <= 0.0 {
            return Some("window_s must be above 0".into());
        }
        if self.window_s > Self::MAX_WINDOW_S {
            return Some(format!(
                "window_s must be at most {} seconds",
                Self::MAX_WINDOW_S
            ));
        }
        if self.interval_s.is_nan() || self.interval_s <= 0.0 {
            return Some("interval_s must be above 0".into());
        }
        if self.interval_s > self.window_s {
            return Some("interval_s must not exceed window_s".into());
        }
        None
    }
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        Self {
            window_s: default_recovery_window_s(),
            interval_s: default_recovery_interval_s(),
        }
    }
}

fn default_recovery_window_s() -> f64 {
    30.0
}

fn default_recovery_interval_s() -> f64 {
    1.0
}

// ── Load Config ────────────────────────────────────────────────
// ── Load Config ────────────────────────────────────────────────

/// Shape of the traffic a load generator produces over `duration_s`. Every
/// profile is turned into the same tool-agnostic plan of stages (see
/// `runner::load_profile`), which each load adapter renders in its own
/// vocabulary; a profile therefore requires `duration_s`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LoadProfile {
    /// Linear ramp from `start_vus` up to `vus` over `ramp_up_s`, then hold
    /// `vus` for the rest of the duration.
    Ramp {
        /// Virtual users at the start of the ramp.
        start_vus: u32,
        /// Seconds the ramp takes to reach `vus`.
        ramp_up_s: f64,
    },
    /// Steady `vus`, a sudden jump to `peak_vus` after `after_s` held for
    /// `hold_s`, then back to `vus` for the remainder.
    Spike {
        /// Virtual users during the spike.
        peak_vus: u32,
        /// Seconds of steady load before the spike.
        after_s: f64,
        /// Seconds the spike is held.
        hold_s: f64,
    },
    /// Constant `vus` for the whole (long) duration, with the tool's
    /// summary sampled every `sample_interval_s` so the run streams a time
    /// series instead of one end-of-run summary.
    Soak {
        /// Seconds between streamed samples.
        sample_interval_s: f64,
    },
}

/// Load test configuration: a load generator run alongside the experiment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadConfig {
    /// Load tool to run.
    pub tool: LoadTool,
    /// Path to the load script executed by the tool. For `oha`, which
    /// takes no script, this is the URL to hit.
    pub script: PathBuf,
    /// Virtual users (passed to the tool when set).
    pub vus: Option<u32>,
    /// Load duration in seconds (passed to the tool when set).
    pub duration_s: Option<f64>,
    /// Tool-specific metric thresholds (metric name → bound).
    #[serde(default)]
    pub thresholds: HashMap<String, f64>,
    /// Traffic shape; `None` is a flat `vus` for `duration_s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<LoadProfile>,
}

// ── Regulatory Mapping ─────────────────────────────────────────

/// A single regulatory requirement an experiment provides evidence for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegulatoryRequirement {
    /// Requirement identifier. A DORA requirement is best named by its
    /// registry id (e.g. `dora.art24.6`); the older forms `DORA-Art24`,
    /// `DORA Art. 24(6)` and a bare `Art. 24` under a DORA mapping read as
    /// the same id (see [`crate::compliance::run_requirement_id`]).
    pub id: String,
    pub description: String,
    /// Short statement of the evidence the experiment provides.
    pub evidence: String,
}

/// Whether a tester is part of the entity or comes from outside it. DORA
/// Art. 24(4) asks that tests are run by independent parties, internal or
/// external.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TesterKind {
    Internal,
    External,
}

impl TesterKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::External => "external",
        }
    }

    /// The kind named `name`; `None` for any other text.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "internal" => Some(Self::Internal),
            "external" => Some(Self::External),
            _ => None,
        }
    }
}

/// Who ran a test: a person, a team or a firm, and its kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tester {
    pub name: String,
    pub kind: TesterKind,
}

/// Regulatory mapping attached to an experiment or `GameDay`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegulatoryMapping {
    /// Framework names or canonical report identifiers (e.g. `DORA`).
    pub frameworks: Vec<String>,
    /// Individual requirements the experiment provides evidence for.
    pub requirements: Vec<RegulatoryRequirement>,
    /// Who runs the test, internal or external (DORA Art. 24(4)). A run
    /// copies it into its journal with the rest of the block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tester: Option<Tester>,
}

// ── Non-functional objectives ──────────────────────────────────

/// One non-functional objective the run is verified against. `metric` names
/// a measurement the runner produces (see `runner::nfr` for the catalogue:
/// `load.latency_p95_ms`, `recovery.mttr_s`, `probe.<name>.error_rate`, …);
/// an objective whose metric was not measured is reported `inconclusive`,
/// never silently met.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NfrObjective {
    /// Stable identifier, unique within the experiment (e.g. `nfr-p95`).
    pub id: String,
    /// What the objective constrains.
    pub kind: NfrKind,
    /// Measurement the objective is evaluated on.
    pub metric: String,
    /// Comparison against `threshold`.
    pub operator: NfrOperator,
    pub threshold: f64,
    /// Evaluation window in seconds (documentation for now; the runner
    /// evaluates over the phase that produced the metric).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_s: Option<f64>,
    /// How much a breach matters; defaults to `major`.
    #[serde(default)]
    pub criticality: NfrCriticality,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

// ── Target ────────────────────────────────────────────────────

/// The system under test, carried on the experiment root span as
/// `resilience.target.*` so the lake can score, scope and gate by target.
/// `system` is the name the scorecard, the gate and compliance reports
/// group by; `environment` is what environment-scoped tokens see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExperimentTarget {
    /// Target system or service name (e.g. `payments-api`).
    pub system: String,
    /// Technology behind it (e.g. `postgresql`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub technology: Option<String>,
    /// Environment the run executes in (e.g. `prod`, `staging`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
    /// Business criticality tier (e.g. `critical`, `important`, `standard`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub criticality: Option<String>,
    /// The target's `service.namespace`, when two services of one name
    /// report to the same store. It narrows which telemetry the observed
    /// version is read from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// The version of the target this run measures (its `service.version`,
    /// e.g. `1.5.0`). Declared here it is recorded on the run as it is;
    /// left out, the runner asks the target's own telemetry at run end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl ExperimentTarget {
    /// The declared version, when it says anything.
    #[must_use]
    pub fn declared_version(&self) -> Option<&str> {
        self.version
            .as_deref()
            .map(str::trim)
            .filter(|version| !version.is_empty())
    }
}

// ── Experiment (the top-level definition) ──────────────────────

/// Top-level experiment definition: identity, configuration, steady-state
/// hypothesis, method (fault window), rollbacks, and per-phase configs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Experiment {
    /// Schema version; defaults to `v1` when omitted.
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Resilience discipline; defaults to `chaos`.
    #[serde(default)]
    pub kind: TestKind,
    /// System under test, emitted as `resilience.target.*` on the root span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<ExperimentTarget>,
    /// Non-functional objectives verified against the run's measurements.
    #[serde(default)]
    pub nfr: Vec<NfrObjective>,
    /// Recovery time objective in seconds (`dr` runs measure against it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rto_target_s: Option<f64>,
    /// Recovery point objective in seconds of tolerated data loss.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpo_target_s: Option<f64>,
    /// How long the post phase watches for recovery; `None` watches for
    /// the default window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<RecoveryConfig>,
    /// Configuration values resolved from the environment (or inlined) at
    /// load time and substituted into provider arguments.
    #[serde(default)]
    pub configuration: IndexMap<String, ConfigValue>,
    /// Secret references (`group -> key -> source`) resolved from the
    /// environment or files at load time; resolved values never enter the
    /// journal.
    #[serde(default)]
    pub secrets: IndexMap<String, IndexMap<String, SecretValue>>,
    #[serde(default)]
    pub controls: Vec<Control>,
    /// Steady-state hypothesis evaluated before and after the method.
    #[serde(default)]
    pub steady_state_hypothesis: Option<Hypothesis>,
    /// Auto-halt guardrails: probes evaluated continuously during the fault
    /// window that pull the plug when their safe-condition tolerance is
    /// breached. Empty (the default) preserves the pre-2.3 behavior exactly.
    #[serde(default)]
    pub guards: Vec<Guard>,
    /// Free-form human note describing the intended blast radius (surfaced in
    /// the journal for audit). Documentation only; not enforced.
    #[serde(default)]
    pub blast_radius: Option<String>,
    /// Cap on concurrently-active background faults during method execution.
    /// Enforced in-process by the runner (see the runner docs for the
    /// enforcement boundary). `None` means unlimited (pre-2.3 behavior).
    #[serde(default)]
    pub max_concurrent_faults: Option<u32>,
    /// Activities executed during the fault window, in declaration order
    /// (background activities run concurrently).
    #[serde(default)]
    pub method: Vec<Activity>,
    /// Activities executed to undo injected faults, per the rollback strategy.
    #[serde(default)]
    pub rollbacks: Vec<Activity>,
    /// Phase 0 prediction, recorded in the journal for analysis.
    #[serde(default)]
    pub estimate: Option<Estimate>,
    /// Phase 1 baseline capture configuration; `None` skips the baseline phase.
    #[serde(default)]
    pub baseline: Option<BaselineConfig>,
    /// Load test to run alongside the method, if any.
    #[serde(default)]
    pub load: Option<LoadConfig>,
    #[serde(default)]
    pub regulatory: Option<RegulatoryMapping>,
}

impl Default for Experiment {
    fn default() -> Self {
        Self {
            version: default_version(),
            title: String::new(),
            description: None,
            tags: vec![],
            kind: TestKind::default(),
            target: None,
            nfr: vec![],
            rto_target_s: None,
            rpo_target_s: None,
            recovery: None,
            configuration: IndexMap::new(),
            secrets: IndexMap::new(),
            controls: vec![],
            steady_state_hypothesis: None,
            guards: vec![],
            blast_radius: None,
            max_concurrent_faults: None,
            method: vec![],
            rollbacks: vec![],
            estimate: None,
            baseline: None,
            load: None,
            regulatory: None,
        }
    }
}

fn default_version() -> String {
    "v1".to_string()
}

/// The structural facts of an experiment that decide whether it can serve
/// as an autopilot playbook. Every gate reads them from
/// [`Experiment::structural_facts`], so the MCP engine, the daemon and the
/// API agree on them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StructuralFacts {
    /// A steady-state hypothesis with at least one probe. Without a probe
    /// the experiment cannot falsify anything.
    pub has_steady_state: bool,
    /// At least one rollback activity.
    pub has_rollback: bool,
    /// At least one guard.
    pub has_guard: bool,
    /// Method actions (fault injections), counting repetitions.
    pub fault_count: usize,
}

impl Experiment {
    /// The [`StructuralFacts`] of this experiment.
    #[must_use]
    pub fn structural_facts(&self) -> StructuralFacts {
        StructuralFacts {
            has_steady_state: self
                .steady_state_hypothesis
                .as_ref()
                .is_some_and(|h| !h.probes.is_empty()),
            has_rollback: !self.rollbacks.is_empty(),
            has_guard: !self.guards.is_empty(),
            fault_count: self
                .method
                .iter()
                .filter(|a| a.activity_type == ActivityType::Action)
                .count(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::types::test_support::toon_round_trip;
    use crate::types::*;
    use indexmap::IndexMap;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn activity_with_label_selector_round_trips() {
        let mut selector = HashMap::new();
        selector.insert("app".to_string(), "api".to_string());
        selector.insert("env".to_string(), "prod".to_string());

        let activity = Activity {
            name: "kill-labeled-pod".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: "tumult-kubernetes".into(),
                function: "delete_pod".into(),
                arguments: HashMap::new(),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: Some(selector),
        };
        let decoded: Activity = toon_round_trip(&activity);
        assert_eq!(decoded, activity);
        assert_eq!(
            decoded.label_selector.as_ref().unwrap().get("app").unwrap(),
            "api"
        );
    }

    #[test]
    fn activity_minimal_round_trips() {
        let activity = Activity {
            name: "kill-pod".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: "tumult-kubernetes".into(),
                function: "delete_pod".into(),
                arguments: HashMap::new(),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        };
        let decoded: Activity = toon_round_trip(&activity);
        assert_eq!(decoded, activity);
    }

    #[test]
    fn activity_with_all_fields_round_trips() {
        let activity = Activity {
            name: "check-health".into(),
            activity_type: ActivityType::Probe,
            provider: Provider::Process {
                path: "scripts/health-check.sh".into(),
                arguments: vec![],
                env: HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: Some(Tolerance::Exact {
                value: serde_json::Value::Number(200.into()),
            }),
            pause_before_s: Some(2.0),
            pause_after_s: Some(5.0),
            background: true,
            label_selector: None,
        };
        let decoded: Activity = toon_round_trip(&activity);
        assert_eq!(decoded, activity);
    }

    #[test]
    fn hypothesis_round_trips() {
        let hypothesis = Hypothesis {
            title: "Application is healthy".into(),
            probes: vec![Activity {
                name: "health-check".into(),
                activity_type: ActivityType::Probe,
                provider: Provider::Process {
                    path: "scripts/health-check.sh".into(),
                    arguments: vec![],
                    env: HashMap::new(),
                    timeout_s: None,
                },
                tolerance: Some(Tolerance::Exact {
                    value: serde_json::Value::Number(200.into()),
                }),
                pause_before_s: None,
                pause_after_s: None,
                background: false,
                label_selector: None,
            }],
        };
        let decoded: Hypothesis = toon_round_trip(&hypothesis);
        assert_eq!(decoded, hypothesis);
    }

    #[test]
    fn estimate_round_trips() {
        let estimate = Estimate {
            expected_outcome: ExpectedOutcome::Recovered,
            expected_recovery_s: Some(15.0),
            expected_degradation: Some(DegradationLevel::Moderate),
            expected_data_loss: Some(false),
            confidence: Some(Confidence::High),
            rationale: Some("Tested monthly, last 5 runs recovered in 10-18s".into()),
            prior_runs: Some(5),
        };
        let decoded: Estimate = toon_round_trip(&estimate);
        assert_eq!(decoded, estimate);
    }

    #[test]
    fn baseline_config_round_trips() {
        let config = BaselineConfig {
            duration_s: 120.0,
            warmup_s: Some(15.0),
            interval_s: 2.0,
            method: BaselineMethod::MeanStddev,
            sigma: Some(2.0),
            confidence: Some(0.95),
        };
        let decoded: BaselineConfig = toon_round_trip(&config);
        assert_eq!(decoded, config);
    }

    #[test]
    fn load_config_round_trips() {
        let config = LoadConfig {
            tool: LoadTool::K6,
            script: PathBuf::from("load/payment-api.js"),
            vus: Some(50),
            duration_s: Some(300.0),
            thresholds: HashMap::from([
                ("http_req_duration_p95".into(), 500.0),
                ("http_req_failed_rate".into(), 0.01),
            ]),
            profile: None,
        };
        let decoded: LoadConfig = toon_round_trip(&config);
        assert_eq!(decoded, config);
    }

    #[test]
    fn regulatory_mapping_round_trips() {
        let mapping = RegulatoryMapping {
            frameworks: vec!["DORA".into(), "PCI-DSS".into()],
            requirements: vec![RegulatoryRequirement {
                id: "DORA-Art24".into(),
                description: "ICT resilience testing programme".into(),
                evidence: "Recovery within RTO".into(),
            }],
            tester: Some(Tester {
                name: "Red Team Ltd".into(),
                kind: TesterKind::External,
            }),
        };
        let decoded: RegulatoryMapping = toon_round_trip(&mapping);
        assert_eq!(decoded, mapping);
    }

    #[test]
    fn a_tester_kind_reads_and_writes_its_name() {
        for (kind, name) in [
            (TesterKind::Internal, "internal"),
            (TesterKind::External, "external"),
        ] {
            assert_eq!(kind.as_str(), name);
            assert_eq!(TesterKind::parse(name), Some(kind));
        }
        assert_eq!(TesterKind::parse("vendor"), None);
    }

    #[test]
    fn a_regulatory_block_without_a_tester_reads_as_before() {
        let text = "frameworks[1]: DORA\nrequirements[1]:\n  - id: Art. 25\n    \
                    description: d\n    evidence: e\n";
        let decoded: RegulatoryMapping = toon_format::decode_default(text).unwrap();
        assert_eq!(decoded.tester, None);
        let named = format!("{text}tester:\n  name: Red Team Ltd\n  kind: external\n");
        let decoded: RegulatoryMapping = toon_format::decode_default(&named).unwrap();
        assert_eq!(decoded.tester.map(|t| t.kind), Some(TesterKind::External));
        let bad = named.replace("external", "vendor");
        assert!(toon_format::decode_default::<RegulatoryMapping>(&bad).is_err());
    }

    #[test]
    fn experiment_minimal_round_trips() {
        let exp = Experiment {
            version: "v1".into(),
            title: "Database failover test".into(),
            description: None,
            tags: vec!["database".into(), "resilience".into()],
            configuration: IndexMap::new(),
            secrets: IndexMap::new(),
            controls: vec![],
            steady_state_hypothesis: None,
            guards: vec![],
            blast_radius: None,
            max_concurrent_faults: None,
            kind: TestKind::default(),
            target: None,
            nfr: vec![],
            rto_target_s: None,
            rpo_target_s: None,
            recovery: None,
            method: vec![],
            rollbacks: vec![],
            estimate: None,
            baseline: None,
            load: None,
            regulatory: None,
        };
        let decoded: Experiment = toon_round_trip(&exp);
        assert_eq!(decoded, exp);
    }

    #[test]
    fn guard_round_trips() {
        let guard = Guard {
            name: "error-rate-slo".into(),
            probe: Activity {
                name: "prom-error-rate".into(),
                activity_type: ActivityType::Probe,
                provider: Provider::Process {
                    path: "curl".into(),
                    arguments: vec!["-s".into(), "http://prom/api/v1/query".into()],
                    env: HashMap::new(),
                    timeout_s: Some(2.0),
                },
                // Safe condition: error rate in [0, 0.05]. Breach ⇒ halt.
                tolerance: Some(Tolerance::Range {
                    from: 0.0,
                    to: 0.05,
                }),
                pause_before_s: None,
                pause_after_s: None,
                background: false,
                label_selector: None,
            },
            min_breaches: 3,
        };
        let decoded: Guard = toon_round_trip(&guard);
        assert_eq!(decoded, guard);
    }

    #[test]
    fn guard_min_breaches_defaults_to_one() {
        // A guard authored without min_breaches decodes to the debounce
        // default of 1 (halt on first breach).
        let toon = "name: g\nprobe:\n  name: p\n  activity_type: probe\n  provider:\n    type: process\n    path: check\n  tolerance:\n    type: range\n    from: 0.0\n    to: 1.0\n";
        let decoded: Guard = toon_format::decode_default(toon).expect("decode guard");
        assert_eq!(decoded.min_breaches, 1);
    }

    #[test]
    fn experiment_with_guards_round_trips() {
        let exp = Experiment {
            version: "v1".into(),
            title: "guarded".into(),
            guards: vec![Guard {
                name: "latency-slo".into(),
                probe: Activity {
                    name: "p95".into(),
                    ..Default::default()
                },
                min_breaches: 2,
            }],
            blast_radius: Some("payments namespace only".into()),
            max_concurrent_faults: Some(2),
            method: vec![Activity {
                name: "inject".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let decoded: Experiment = toon_round_trip(&exp);
        assert_eq!(decoded, exp);
    }

    #[test]
    fn kind_defaults_to_chaos_when_omitted() {
        let exp: Experiment = toon_format::decode_default("title: plain\n").expect("parse");
        assert_eq!(exp.kind, TestKind::Chaos);
        assert!(exp.nfr.is_empty());
        assert_eq!(exp.rto_target_s, None);
    }

    #[test]
    fn kind_nfr_and_recovery_targets_parse_and_round_trip() {
        let toon = "title: failover\nkind: dr\nrto_target_s: 900\nrpo_target_s: 60\nnfr[2]:\n  - id: p95\n    kind: latency\n    metric: load.latency_p95_ms\n    operator: lte\n    threshold: 250\n    criticality: blocking\n  - id: avail\n    kind: availability\n    metric: during.availability\n    operator: gte\n    threshold: 0.999\n";
        let exp: Experiment = toon_format::decode_default(toon).expect("parse");
        assert_eq!(exp.kind, TestKind::Dr);
        assert_eq!(exp.rto_target_s, Some(900.0));
        assert_eq!(exp.rpo_target_s, Some(60.0));
        assert_eq!(exp.nfr.len(), 2);
        assert_eq!(exp.nfr[0].criticality, NfrCriticality::Blocking);
        assert_eq!(exp.nfr[1].criticality, NfrCriticality::Major);
        assert_eq!(exp.nfr[1].operator, NfrOperator::Gte);
        let back = toon_round_trip(&exp);
        assert_eq!(back, exp);
    }

    #[test]
    fn a_recovery_block_parses_with_its_defaults_and_an_old_file_has_none() {
        let toon = "title: t\nkind: dr\nrto_target_s: 60\nrecovery:\n  window_s: 60\n";
        let exp: Experiment = toon_format::decode_default(toon).expect("parse");
        assert_eq!(
            exp.recovery,
            Some(RecoveryConfig {
                window_s: 60.0,
                interval_s: 1.0
            })
        );
        assert_eq!(toon_round_trip(&exp), exp);

        let both = "title: t\nrecovery:\n  window_s: 120\n  interval_s: 5\n";
        let exp: Experiment = toon_format::decode_default(both).expect("parse");
        assert_eq!(
            exp.recovery,
            Some(RecoveryConfig {
                window_s: 120.0,
                interval_s: 5.0
            })
        );
        let interval_only = "title: t\nrecovery:\n  interval_s: 2\n";
        let exp: Experiment = toon_format::decode_default(interval_only).expect("parse");
        assert_eq!(exp.recovery.map(|r| r.window_s), Some(30.0));

        // A file written before the block reads as it did, and writes no
        // block.
        let old: Experiment = toon_format::decode_default("title: t\n").expect("parse");
        assert_eq!(old.recovery, None);
        let written = toon_format::encode_default(&old).expect("encode");
        assert!(!written.contains("recovery"), "{written}");

        // A key the block does not know is an error.
        let unknown = "title: t\nrecovery:\n  timeout_s: 90\n";
        assert!(toon_format::decode_default::<Experiment>(unknown).is_err());
    }

    #[test]
    fn a_target_may_declare_the_version_it_measures() {
        let toon = "title: t\ntarget:\n  system: checkout\n  environment: demo\n  version: 1.5.0\n";
        let exp: Experiment = toon_format::decode_default(toon).expect("parse");
        let target = exp.target.clone().expect("target");
        assert_eq!(target.version.as_deref(), Some("1.5.0"));
        assert_eq!(target.declared_version(), Some("1.5.0"));
        assert_eq!(toon_round_trip(&exp), exp);

        // A blank version declares nothing.
        let blank = ExperimentTarget {
            version: Some("  ".into()),
            ..target.clone()
        };
        assert_eq!(blank.declared_version(), None);
        let none = ExperimentTarget {
            version: None,
            ..target
        };
        assert_eq!(none.declared_version(), None);
    }

    #[test]
    fn target_block_parses_and_round_trips() {
        let toon = "title: t\ntarget:\n  system: payments-api\n  technology: postgresql\n  environment: prod\n";
        let exp: Experiment = toon_format::decode_default(toon).expect("parse");
        let target = exp.target.as_ref().expect("target");
        assert_eq!(target.system, "payments-api");
        assert_eq!(target.environment.as_deref(), Some("prod"));
        assert_eq!(target.criticality, None);
        assert_eq!(toon_round_trip(&exp), exp);
        assert!(
            toon_format::decode_default::<Experiment>("title: t\ntarget:\n  name: x\n").is_err()
        );
    }

    #[test]
    fn load_profiles_parse_and_round_trip() {
        let ramp = "title: t\nload:\n  tool: k6\n  script: load.js\n  vus: 50\n  duration_s: 120\n  profile:\n    type: ramp\n    start_vus: 5\n    ramp_up_s: 30\n";
        let exp: Experiment = toon_format::decode_default(ramp).expect("parse ramp");
        let load = exp.load.as_ref().expect("load");
        assert_eq!(
            load.profile,
            Some(LoadProfile::Ramp {
                start_vus: 5,
                ramp_up_s: 30.0
            })
        );
        assert_eq!(toon_round_trip(&exp), exp);

        let spike = "title: t\nload:\n  tool: oha\n  script: http://localhost:8080/health\n  vus: 10\n  duration_s: 60\n  profile:\n    type: spike\n    peak_vus: 200\n    after_s: 20\n    hold_s: 10\n";
        let exp: Experiment = toon_format::decode_default(spike).expect("parse spike");
        let load = exp.load.as_ref().expect("load");
        assert_eq!(load.tool, LoadTool::Oha);
        assert!(matches!(
            load.profile,
            Some(LoadProfile::Spike { peak_vus: 200, .. })
        ));
        assert_eq!(toon_round_trip(&exp), exp);

        let soak = "title: t\nload:\n  tool: oha\n  script: http://localhost:8080/\n  vus: 20\n  duration_s: 3600\n  profile:\n    type: soak\n    sample_interval_s: 10\n";
        let exp: Experiment = toon_format::decode_default(soak).expect("parse soak");
        assert!(matches!(
            exp.load.as_ref().and_then(|l| l.profile.as_ref()),
            Some(LoadProfile::Soak { .. })
        ));
    }

    #[test]
    fn load_without_profile_still_parses_and_omits_the_field() {
        let toon = "title: t\nload:\n  tool: k6\n  script: load.js\n";
        let exp: Experiment = toon_format::decode_default(toon).expect("parse");
        assert_eq!(exp.load.as_ref().and_then(|l| l.profile.clone()), None);
        let encoded = toon_format::encode_default(&exp).expect("encode");
        assert!(!encoded.contains("profile"), "{encoded}");
    }

    #[test]
    fn unknown_load_profile_field_or_type_is_rejected() {
        let bogus_field = "title: t\nload:\n  tool: k6\n  script: load.js\n  duration_s: 60\n  profile:\n    type: ramp\n    start_vus: 1\n    ramp_up_s: 5\n    bogus: 1\n";
        assert!(toon_format::decode_default::<Experiment>(bogus_field).is_err());
        let bogus_type =
            "title: t\nload:\n  tool: k6\n  script: load.js\n  profile:\n    type: burst\n";
        assert!(toon_format::decode_default::<Experiment>(bogus_type).is_err());
    }

    #[test]
    fn unknown_nfr_field_is_rejected() {
        let toon = "title: x\nnfr[1]:\n  - id: a\n    kind: latency\n    metric: m\n    operator: lte\n    threshold: 1\n    bogus: 2\n";
        assert!(toon_format::decode_default::<Experiment>(toon).is_err());
    }

    const STEP: &str = "title: t\nmethod[1]:\n  - name: step\n    activity_type: action\n    provider:\n      type: process\n      path: echo\n";

    fn parse_error(toon: &str) -> String {
        toon_format::decode_default::<Experiment>(toon)
            .expect_err("an unknown key must be an error")
            .to_string()
    }

    #[test]
    fn a_plain_step_parses() {
        let exp: Experiment = toon_format::decode_default(STEP).expect("parse");
        assert_eq!(exp.method.len(), 1);
    }

    #[test]
    fn unknown_activity_key_is_rejected_and_named() {
        // The plugin docs used to show this key. It was dropped without a
        // word, so a step written for a remote host ran on the local one.
        let toon = format!("{STEP}    execution_target:\n      type: ssh\n      host: db\n");
        let error = parse_error(&toon);
        assert!(error.contains("execution_target"), "{error}");
    }

    #[test]
    fn structural_facts_count_faults_and_want_a_probe_for_a_steady_state() {
        let action = |name: &str| Activity {
            name: name.into(),
            activity_type: ActivityType::Action,
            ..Activity::default()
        };
        let probe = Activity {
            activity_type: ActivityType::Probe,
            ..action("look")
        };
        let mut exp = Experiment {
            steady_state_hypothesis: Some(Hypothesis {
                title: "up".into(),
                probes: vec![],
            }),
            method: vec![action("a"), probe.clone(), action("a")],
            rollbacks: vec![action("undo")],
            ..Experiment::default()
        };
        assert_eq!(
            exp.structural_facts(),
            StructuralFacts {
                has_steady_state: false,
                has_rollback: true,
                has_guard: false,
                fault_count: 2,
            },
            "a hypothesis without a probe is no steady state"
        );
        exp.steady_state_hypothesis = Some(Hypothesis {
            title: "up".into(),
            probes: vec![probe],
        });
        assert!(exp.structural_facts().has_steady_state);
        assert_eq!(
            Experiment::default().structural_facts(),
            StructuralFacts::default()
        );
    }

    #[test]
    fn unknown_provider_key_is_rejected_and_named() {
        let error = parse_error(&format!("{STEP}      timeout: 5\n"));
        assert!(error.contains("timeout"), "{error}");
    }

    #[test]
    fn unknown_tolerance_key_is_rejected_and_named() {
        let toon = format!(
            "{STEP}    tolerance:\n      type: range\n      from: 0\n      to: 1\n      unit: ms\n"
        );
        let error = parse_error(&toon);
        assert!(error.contains("unit"), "{error}");
    }
}
