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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    pub name: String,
    pub activity_type: ActivityType,
    pub provider: Provider,
    #[serde(default)]
    pub tolerance: Option<Tolerance>,
    #[serde(default)]
    pub pause_before_s: Option<f64>,
    #[serde(default)]
    pub pause_after_s: Option<f64>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hypothesis {
    pub title: String,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub name: String,
    pub provider: Provider,
}

// ── Estimate (Phase 0) ────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Estimate {
    pub expected_outcome: ExpectedOutcome,
    pub expected_recovery_s: Option<f64>,
    pub expected_degradation: Option<DegradationLevel>,
    pub expected_data_loss: Option<bool>,
    pub confidence: Option<Confidence>,
    pub rationale: Option<String>,
    pub prior_runs: Option<u32>,
}

// ── Baseline Config (Phase 1) ──────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BaselineConfig {
    pub duration_s: f64,
    pub warmup_s: Option<f64>,
    pub interval_s: f64,
    pub method: BaselineMethod,
    pub sigma: Option<f64>,
    pub confidence: Option<f64>,
}

// ── Load Config ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoadConfig {
    pub tool: LoadTool,
    pub script: PathBuf,
    pub vus: Option<u32>,
    pub duration_s: Option<f64>,
    #[serde(default)]
    pub thresholds: HashMap<String, f64>,
}

// ── Regulatory Mapping ─────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegulatoryRequirement {
    pub id: String,
    pub description: String,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegulatoryMapping {
    pub frameworks: Vec<String>,
    pub requirements: Vec<RegulatoryRequirement>,
}

// ── Experiment (the top-level definition) ──────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Experiment {
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub configuration: IndexMap<String, ConfigValue>,
    #[serde(default)]
    pub secrets: IndexMap<String, IndexMap<String, SecretValue>>,
    #[serde(default)]
    pub controls: Vec<Control>,
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
    #[serde(default)]
    pub method: Vec<Activity>,
    #[serde(default)]
    pub rollbacks: Vec<Activity>,
    #[serde(default)]
    pub estimate: Option<Estimate>,
    #[serde(default)]
    pub baseline: Option<BaselineConfig>,
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
