//! Experiment authoring types: the top-level `Experiment` and its sections.

use std::collections::HashMap;
use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use super::enums::{
    ActivityType, BaselineMethod, Confidence, DegradationLevel, ExpectedOutcome, LoadTool,
};
use super::target::{ConfigValue, Provider, SecretValue, Tolerance};

// ── Activity ───────────────────────────────────────────────────

/// A single experiment step: an action (fault injection) or a probe
/// (measurement), executed through a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

// ── Load Config ────────────────────────────────────────────────

/// Load test configuration: a load generator run alongside the experiment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadConfig {
    /// Load tool to run.
    pub tool: LoadTool,
    /// Path to the load script executed by the tool.
    pub script: PathBuf,
    /// Virtual users (passed to the tool when set).
    pub vus: Option<u32>,
    /// Load duration in seconds (passed to the tool when set).
    pub duration_s: Option<f64>,
    /// Tool-specific metric thresholds (metric name → bound).
    #[serde(default)]
    pub thresholds: HashMap<String, f64>,
}

// ── Regulatory Mapping ─────────────────────────────────────────

/// A single regulatory requirement an experiment provides evidence for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegulatoryRequirement {
    /// Canonical requirement identifier (e.g. `DORA-Art24`).
    pub id: String,
    pub description: String,
    /// Short statement of the evidence the experiment provides.
    pub evidence: String,
}

/// Regulatory mapping attached to an experiment or `GameDay`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegulatoryMapping {
    /// Framework names or canonical report identifiers (e.g. `DORA`).
    pub frameworks: Vec<String>,
    /// Individual requirements the experiment provides evidence for.
    pub requirements: Vec<RegulatoryRequirement>,
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
        };
        let decoded: RegulatoryMapping = toon_round_trip(&mapping);
        assert_eq!(decoded, mapping);
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
}
