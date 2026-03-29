//! Core data model types for Tumult experiments.
//!
//! All types derive `serde::Serialize` and `serde::Deserialize`
//! for round-trip TOON serialization.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ── Enums ──────────────────────────────────────────────────────

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
#[serde(rename_all = "UPPERCASE")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Patch,
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

// ── Execution Target ───────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecutionTarget {
    Local,
    Ssh {
        host: String,
        port: u16,
        user: String,
        key_path: Option<PathBuf>,
    },
    Container {
        runtime: ContainerRuntime,
        container_id: String,
    },
    KubeExec {
        namespace: String,
        pod: String,
        container: Option<String>,
    },
}

// ── Provider ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Provider {
    Native {
        plugin: String,
        function: String,
        arguments: HashMap<String, serde_json::Value>,
    },
    Process {
        path: String,
        arguments: Vec<String>,
        env: HashMap<String, String>,
        timeout_s: Option<f64>,
    },
    Http {
        method: HttpMethod,
        url: String,
        headers: HashMap<String, String>,
        body: Option<String>,
        timeout_s: Option<f64>,
    },
}

// ── Tolerance ──────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Tolerance {
    Exact { value: serde_json::Value },
    Range { from: f64, to: f64 },
    Regex { pattern: String },
}

// ── Config and Secrets ─────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConfigValue {
    Env { key: String },
    Inline { value: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SecretValue {
    Env { key: String },
    File { path: PathBuf },
}

// ── Activity ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Activity {
    pub name: String,
    pub activity_type: ActivityType,
    pub provider: Provider,
    pub tolerance: Option<Tolerance>,
    pub pause_before_s: Option<f64>,
    pub pause_after_s: Option<f64>,
    pub background: bool,
}

// ── Hypothesis ─────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hypothesis {
    pub title: String,
    pub probes: Vec<Activity>,
}

// ── Control ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Control {
    pub name: String,
    pub provider: Provider,
}

// ── Estimate (Phase 0) ────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
pub struct LoadConfig {
    pub tool: LoadTool,
    pub script: PathBuf,
    pub vus: Option<u32>,
    pub duration_s: Option<f64>,
    pub thresholds: HashMap<String, f64>,
}

// ── Regulatory Mapping ─────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegulatoryRequirement {
    pub id: String,
    pub description: String,
    pub evidence: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegulatoryMapping {
    pub frameworks: Vec<String>,
    pub requirements: Vec<RegulatoryRequirement>,
}

// ── Experiment (the top-level definition) ──────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Experiment {
    pub title: String,
    pub description: Option<String>,
    pub tags: Vec<String>,
    pub configuration: HashMap<String, ConfigValue>,
    pub secrets: HashMap<String, HashMap<String, SecretValue>>,
    pub controls: Vec<Control>,
    pub steady_state_hypothesis: Option<Hypothesis>,
    pub method: Vec<Activity>,
    pub rollbacks: Vec<Activity>,
    pub estimate: Option<Estimate>,
    pub baseline: Option<BaselineConfig>,
    pub load: Option<LoadConfig>,
    pub regulatory: Option<RegulatoryMapping>,
}

// ── Journal types (experiment output) ──────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityResult {
    pub name: String,
    pub activity_type: ActivityType,
    pub status: ActivityStatus,
    pub started_at_ns: i64,
    pub duration_ms: u64,
    pub output: Option<String>,
    pub error: Option<String>,
    pub trace_id: String,
    pub span_id: String,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    // ── Helper: round-trip through TOON ────────────────────────

    fn toon_round_trip<T>(value: &T) -> T
    where
        T: serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug,
    {
        let encoded = toon_format::encode_default(value).expect("TOON encode failed");
        toon_format::decode_default(&encoded).expect("TOON decode failed")
    }

    // ── ActivityType ───────────────────────────────────────────

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

    // ── ExperimentStatus ───────────────────────────────────────

    #[test]
    fn experiment_status_all_variants_round_trip() {
        for status in [
            ExperimentStatus::Completed,
            ExperimentStatus::Deviated,
            ExperimentStatus::Aborted,
            ExperimentStatus::Failed,
            ExperimentStatus::Interrupted,
        ] {
            let decoded: ExperimentStatus = toon_round_trip(&status);
            assert_eq!(decoded, status);
        }
    }

    // ── ActivityStatus ─────────────────────────────────────────

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

    // ── HttpMethod ─────────────────────────────────────────────

    #[test]
    fn http_method_all_variants_round_trip() {
        for method in [
            HttpMethod::Get,
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Delete,
            HttpMethod::Patch,
        ] {
            let decoded: HttpMethod = toon_round_trip(&method);
            assert_eq!(decoded, method);
        }
    }

    // ── ContainerRuntime ───────────────────────────────────────

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

    // ── ExecutionTarget ────────────────────────────────────────

    #[test]
    fn execution_target_local_round_trips() {
        let target = ExecutionTarget::Local;
        let decoded: ExecutionTarget = toon_round_trip(&target);
        assert_eq!(decoded, ExecutionTarget::Local);
    }

    #[test]
    fn execution_target_ssh_round_trips() {
        let target = ExecutionTarget::Ssh {
            host: "db-primary.example.com".into(),
            port: 22,
            user: "ops".into(),
            key_path: Some(PathBuf::from("/home/ops/.ssh/id_ed25519")),
        };
        let decoded: ExecutionTarget = toon_round_trip(&target);
        assert_eq!(decoded, target);
    }

    #[test]
    fn execution_target_container_round_trips() {
        let target = ExecutionTarget::Container {
            runtime: ContainerRuntime::Docker,
            container_id: "abc123def456".into(),
        };
        let decoded: ExecutionTarget = toon_round_trip(&target);
        assert_eq!(decoded, target);
    }

    #[test]
    fn execution_target_kube_exec_round_trips() {
        let target = ExecutionTarget::KubeExec {
            namespace: "production".into(),
            pod: "api-server-7b8c9d-xk2p1".into(),
            container: Some("app".into()),
        };
        let decoded: ExecutionTarget = toon_round_trip(&target);
        assert_eq!(decoded, target);
    }

    // ── Provider ───────────────────────────────────────────────

    #[test]
    fn provider_native_round_trips() {
        let provider = Provider::Native {
            plugin: "tumult-db".into(),
            function: "terminate_connections".into(),
            arguments: HashMap::from([
                ("host".into(), serde_json::Value::String("localhost".into())),
                ("port".into(), serde_json::Value::Number(5432.into())),
            ]),
        };
        let decoded: Provider = toon_round_trip(&provider);
        assert_eq!(decoded, provider);
    }

    #[test]
    fn provider_process_round_trips() {
        let provider = Provider::Process {
            path: "scripts/kill-broker.sh".into(),
            arguments: vec!["--broker-id".into(), "2".into()],
            env: HashMap::from([("CLUSTER".into(), "prod".into())]),
            timeout_s: Some(30.0),
        };
        let decoded: Provider = toon_round_trip(&provider);
        assert_eq!(decoded, provider);
    }

    #[test]
    fn provider_http_round_trips() {
        let provider = Provider::Http {
            method: HttpMethod::Get,
            url: "http://localhost:8080/health".into(),
            headers: HashMap::from([("Accept".into(), "application/json".into())]),
            body: None,
            timeout_s: Some(5.0),
        };
        let decoded: Provider = toon_round_trip(&provider);
        assert_eq!(decoded, provider);
    }

    // ── Tolerance ──────────────────────────────────────────────

    #[test]
    fn tolerance_exact_round_trips() {
        let t = Tolerance::Exact {
            value: serde_json::Value::Number(200.into()),
        };
        let decoded: Tolerance = toon_round_trip(&t);
        assert_eq!(decoded, t);
    }

    #[test]
    fn tolerance_range_round_trips() {
        let t = Tolerance::Range {
            from: 0.0,
            to: 500.0,
        };
        let decoded: Tolerance = toon_round_trip(&t);
        assert_eq!(decoded, t);
    }

    #[test]
    fn tolerance_regex_round_trips() {
        let t = Tolerance::Regex {
            pattern: "^OK.*".into(),
        };
        let decoded: Tolerance = toon_round_trip(&t);
        assert_eq!(decoded, t);
    }

    // ── Activity ───────────────────────────────────────────────

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
        };
        let decoded: Activity = toon_round_trip(&activity);
        assert_eq!(decoded, activity);
    }

    #[test]
    fn activity_with_all_fields_round_trips() {
        let activity = Activity {
            name: "check-health".into(),
            activity_type: ActivityType::Probe,
            provider: Provider::Http {
                method: HttpMethod::Get,
                url: "http://app:8080/health".into(),
                headers: HashMap::new(),
                body: None,
                timeout_s: Some(5.0),
            },
            tolerance: Some(Tolerance::Exact {
                value: serde_json::Value::Number(200.into()),
            }),
            pause_before_s: Some(2.0),
            pause_after_s: Some(5.0),
            background: true,
        };
        let decoded: Activity = toon_round_trip(&activity);
        assert_eq!(decoded, activity);
    }

    // ── Hypothesis ─────────────────────────────────────────────

    #[test]
    fn hypothesis_round_trips() {
        let hypothesis = Hypothesis {
            title: "Application is healthy".into(),
            probes: vec![Activity {
                name: "health-check".into(),
                activity_type: ActivityType::Probe,
                provider: Provider::Http {
                    method: HttpMethod::Get,
                    url: "http://app:8080/health".into(),
                    headers: HashMap::new(),
                    body: None,
                    timeout_s: None,
                },
                tolerance: Some(Tolerance::Exact {
                    value: serde_json::Value::Number(200.into()),
                }),
                pause_before_s: None,
                pause_after_s: None,
                background: false,
            }],
        };
        let decoded: Hypothesis = toon_round_trip(&hypothesis);
        assert_eq!(decoded, hypothesis);
    }

    // ── Estimate ───────────────────────────────────────────────

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

    // ── BaselineConfig ─────────────────────────────────────────

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

    // ── LoadConfig ─────────────────────────────────────────────

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

    // ── RegulatoryMapping ──────────────────────────────────────

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

    // ── Experiment (full) ──────────────────────────────────────

    #[test]
    fn experiment_minimal_round_trips() {
        let exp = Experiment {
            title: "Database failover test".into(),
            description: None,
            tags: vec!["database".into(), "resilience".into()],
            configuration: HashMap::new(),
            secrets: HashMap::new(),
            controls: vec![],
            steady_state_hypothesis: None,
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

    // ── ActivityResult ─────────────────────────────────────────

    #[test]
    fn activity_result_round_trips() {
        let result = ActivityResult {
            name: "kill-pod".into(),
            activity_type: ActivityType::Action,
            status: ActivityStatus::Succeeded,
            started_at_ns: 1774980135342000000,
            duration_ms: 342,
            output: Some("pod deleted".into()),
            error: None,
            trace_id: "abc123".into(),
            span_id: "def456".into(),
        };
        let decoded: ActivityResult = toon_round_trip(&result);
        assert_eq!(decoded, result);
    }

    // ── Journal ────────────────────────────────────────────────

    #[test]
    fn journal_minimal_round_trips() {
        let journal = Journal {
            experiment_title: "Database failover test".into(),
            experiment_id: "550e8400-e29b-41d4-a716-446655440000".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1774980000000000000,
            ended_at_ns: 1774980300000000000,
            duration_ms: 300000,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![],
            rollback_results: vec![],
        };
        let decoded: Journal = toon_round_trip(&journal);
        assert_eq!(decoded, journal);
    }
}
