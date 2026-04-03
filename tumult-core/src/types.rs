//! Core data model types for Tumult experiments.
//!
//! All types derive `serde::Serialize` and `serde::Deserialize`
//! for round-trip TOON serialization.

use std::collections::HashMap;
use std::path::PathBuf;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

// ── Newtype identifiers ────────────────────────────────────────

/// A newtype wrapper for OpenTelemetry trace IDs.
///
/// Stored as a hex string (e.g. `"4bf92f3577b34da6a3ce929d0e0e4736"`).
/// Empty string signals no active trace (noop tracer or uninstrumented path).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TraceId(pub String);

impl TraceId {
    /// Creates an empty (no-trace) identifier.
    #[must_use]
    pub fn empty() -> Self {
        Self(String::new())
    }

    /// Returns `true` if the trace ID is empty (noop tracer or uninstrumented).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the inner string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TraceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for TraceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TraceId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl AsRef<str> for TraceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A newtype wrapper for OpenTelemetry span IDs.
///
/// Stored as a hex string (e.g. `"00f067aa0ba902b7"`).
/// Empty string signals no active span (noop tracer or uninstrumented path).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpanId(pub String);

impl SpanId {
    /// Creates an empty (no-span) identifier.
    #[must_use]
    pub fn empty() -> Self {
        Self(String::new())
    }

    /// Returns `true` if the span ID is empty (noop tracer or uninstrumented).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the inner string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SpanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SpanId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SpanId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl AsRef<str> for SpanId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

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

impl std::fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Get => write!(f, "GET"),
            Self::Post => write!(f, "POST"),
            Self::Put => write!(f, "PUT"),
            Self::Delete => write!(f, "DELETE"),
            Self::Patch => write!(f, "PATCH"),
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
        /// Optional label selector for filtering containers by Docker/Podman labels.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label_selector: Option<HashMap<String, String>>,
    },
    KubeExec {
        namespace: String,
        pod: String,
        container: Option<String>,
        /// Optional label selector for targeting pods by Kubernetes labels.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label_selector: Option<HashMap<String, String>>,
    },
}

// ── Provider ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Provider {
    Native {
        plugin: String,
        function: String,
        #[serde(default)]
        arguments: HashMap<String, serde_json::Value>,
    },
    Process {
        path: String,
        #[serde(default)]
        arguments: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
        timeout_s: Option<f64>,
    },
    Http {
        method: HttpMethod,
        url: String,
        #[serde(default)]
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

// ── Enums for result phases ─────────────────────────────────────

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

// ── Baseline Result (Phase 1 output) ───────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeBaseline {
    pub name: String,
    pub mean: f64,
    pub stddev: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub min: f64,
    pub max: f64,
    pub error_rate: f64,
    pub samples: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BaselineResult {
    pub started_at_ns: i64,
    pub ended_at_ns: i64,
    pub duration_s: f64,
    pub warmup_s: f64,
    pub samples: u32,
    pub interval_s: f64,
    pub method: BaselineMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sigma: Option<f64>,
    pub source: BaselineSource,
    pub anomaly_detected: bool,
    pub probes: Vec<ProbeBaseline>,
    pub tolerance_lower: f64,
    pub tolerance_upper: f64,
}

// ── During Result (Phase 2 output) ─────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbeDuring {
    pub name: String,
    pub samples: u32,
    pub mean: f64,
    pub max: f64,
    pub min: f64,
    pub error_rate: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breached_at_ns: Option<i64>,
    pub breach_count: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DuringResult {
    pub started_at_ns: i64,
    pub ended_at_ns: i64,
    pub fault_active_s: f64,
    pub sample_interval_s: f64,
    pub probes: Vec<ProbeDuring>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degradation_onset_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degradation_peak_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degradation_magnitude: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graceful_degradation: Option<bool>,
}

// ── Post Result (Phase 3 output) ───────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProbePost {
    pub name: String,
    pub mean: f64,
    pub p95: f64,
    pub error_rate: f64,
    pub returned_to_baseline: bool,
    pub recovery_time_s: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PostResult {
    pub started_at_ns: i64,
    pub ended_at_ns: i64,
    pub duration_s: f64,
    pub samples: u32,
    pub probes: Vec<ProbePost>,
    pub recovery_time_s: f64,
    pub full_recovery: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub residual_degradation: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_integrity_verified: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_loss_detected: Option<bool>,
    /// Mean time to recovery in seconds.
    ///
    /// Measured as elapsed time from `started_at_ns` (method end) until the
    /// first probe sample that falls within baseline tolerance. `None` if
    /// recovery was never observed in the post-phase window or no samples
    /// were taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mttr_s: Option<f64>,
}

// ── Load Result ────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoadResult {
    pub tool: LoadTool,
    pub started_at_ns: i64,
    pub ended_at_ns: i64,
    pub duration_s: f64,
    pub vus: u32,
    pub throughput_rps: f64,
    pub latency_p50_ms: f64,
    pub latency_p95_ms: f64,
    pub latency_p99_ms: f64,
    pub error_rate: f64,
    pub total_requests: u64,
    pub thresholds_met: bool,
}

// ── Analysis Result (Phase 4 output) ───────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnalysisResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_accuracy: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_recovery_delta_s: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trend: Option<Trend>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resilience_score: Option<f64>,
}

// ── Journal types (experiment output) ──────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActivityResult {
    pub name: String,
    pub activity_type: ActivityType,
    pub status: ActivityStatus,
    pub started_at_ns: i64,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub trace_id: TraceId,
    pub span_id: SpanId,
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
    /// Number of rollback activities that failed during execution.
    #[serde(default)]
    pub rollback_failures: u32,
    pub estimate: Option<Estimate>,
    pub baseline_result: Option<BaselineResult>,
    pub during_result: Option<DuringResult>,
    pub post_result: Option<PostResult>,
    pub load_result: Option<LoadResult>,
    pub analysis: Option<AnalysisResult>,
    pub regulatory: Option<RegulatoryMapping>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    // ── Full experiment construction + TOON round-trip ────────

    fn build_sample_experiment() -> Experiment {
        Experiment {
            version: "v1".into(),
            title: "Database failover validates automatic reconnection".into(),
            description: Some("Kill PostgreSQL primary and verify app reconnects".into()),
            tags: vec!["database".into(), "resilience".into()],
            configuration: IndexMap::from([(
                "db_host".into(),
                ConfigValue::Env {
                    key: "DATABASE_HOST".into(),
                },
            )]),
            secrets: IndexMap::new(),
            controls: vec![],
            steady_state_hypothesis: Some(Hypothesis {
                title: "Application responds healthy".into(),
                probes: vec![Activity {
                    name: "health-check".into(),
                    activity_type: ActivityType::Probe,
                    provider: Provider::Http {
                        method: HttpMethod::Get,
                        url: "http://localhost:8080/health".into(),
                        headers: HashMap::new(),
                        body: None,
                        timeout_s: Some(5.0),
                    },
                    tolerance: Some(Tolerance::Exact {
                        value: serde_json::Value::Number(200.into()),
                    }),
                    pause_before_s: None,
                    pause_after_s: None,
                    background: false,
                    label_selector: None,
                }],
            }),
            method: vec![Activity {
                name: "kill-db-connections".into(),
                activity_type: ActivityType::Action,
                provider: Provider::Native {
                    plugin: "tumult-db".into(),
                    function: "terminate_connections".into(),
                    arguments: HashMap::from([(
                        "database".into(),
                        serde_json::Value::String("myapp".into()),
                    )]),
                },
                tolerance: None,
                pause_before_s: None,
                pause_after_s: Some(5.0),
                background: false,
                label_selector: None,
            }],
            rollbacks: vec![Activity {
                name: "restore-connections".into(),
                activity_type: ActivityType::Action,
                provider: Provider::Native {
                    plugin: "tumult-db".into(),
                    function: "reset_connection_pool".into(),
                    arguments: HashMap::new(),
                },
                tolerance: None,
                pause_before_s: None,
                pause_after_s: None,
                background: false,
                label_selector: None,
            }],
            estimate: Some(Estimate {
                expected_outcome: ExpectedOutcome::Recovered,
                expected_recovery_s: Some(15.0),
                expected_degradation: Some(DegradationLevel::Moderate),
                expected_data_loss: Some(false),
                confidence: Some(Confidence::High),
                rationale: Some("Tested monthly with consistent recovery".into()),
                prior_runs: Some(5),
            }),
            baseline: Some(BaselineConfig {
                duration_s: 120.0,
                warmup_s: Some(15.0),
                interval_s: 2.0,
                method: BaselineMethod::MeanStddev,
                sigma: Some(2.0),
                confidence: Some(0.95),
            }),
            load: None,
            regulatory: Some(RegulatoryMapping {
                frameworks: vec!["DORA".into()],
                requirements: vec![RegulatoryRequirement {
                    id: "DORA-Art24".into(),
                    description: "ICT resilience testing".into(),
                    evidence: "Recovery within RTO".into(),
                }],
            }),
        }
    }

    #[test]
    fn full_experiment_round_trips_through_toon() {
        let exp = build_sample_experiment();
        let decoded: Experiment = toon_round_trip(&exp);
        assert_eq!(decoded, exp);
    }

    #[test]
    fn full_experiment_has_all_sections() {
        let exp = build_sample_experiment();
        assert_eq!(
            exp.title,
            "Database failover validates automatic reconnection"
        );
        assert_eq!(exp.tags, vec!["database", "resilience"]);
        assert!(exp.estimate.is_some());
        assert!(exp.baseline.is_some());
        assert!(exp.regulatory.is_some());
        assert!(exp.steady_state_hypothesis.is_some());
        assert_eq!(exp.method.len(), 1);
        assert_eq!(exp.rollbacks.len(), 1);
        assert_eq!(exp.method[0].activity_type, ActivityType::Action);
        assert_eq!(
            exp.steady_state_hypothesis.as_ref().unwrap().probes[0].activity_type,
            ActivityType::Probe
        );
    }

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
            label_selector: None,
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
            label_selector: None,
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

    // ── label_selector additions ───────────────────────────────

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
    fn execution_target_kube_exec_with_label_selector_round_trips() {
        let mut selector = HashMap::new();
        selector.insert("app".to_string(), "worker".to_string());

        let target = ExecutionTarget::KubeExec {
            namespace: "production".into(),
            pod: String::new(),
            container: None,
            label_selector: Some(selector),
        };
        let decoded: ExecutionTarget = toon_round_trip(&target);
        assert_eq!(decoded, target);
        let ExecutionTarget::KubeExec { label_selector, .. } = &decoded else {
            panic!("expected KubeExec");
        };
        assert_eq!(
            label_selector.as_ref().unwrap().get("app").unwrap(),
            "worker"
        );
    }

    #[test]
    fn execution_target_container_with_label_selector_round_trips() {
        let mut selector = HashMap::new();
        selector.insert(
            "com.docker.compose.service".to_string(),
            "redis".to_string(),
        );

        let target = ExecutionTarget::Container {
            runtime: ContainerRuntime::Docker,
            container_id: String::new(),
            label_selector: Some(selector),
        };
        let decoded: ExecutionTarget = toon_round_trip(&target);
        assert_eq!(decoded, target);
        let ExecutionTarget::Container { label_selector, .. } = &decoded else {
            panic!("expected Container");
        };
        assert_eq!(
            label_selector
                .as_ref()
                .unwrap()
                .get("com.docker.compose.service")
                .unwrap(),
            "redis"
        );
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
            label_selector: None,
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
                label_selector: None,
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
            version: "v1".into(),
            title: "Database failover test".into(),
            description: None,
            tags: vec!["database".into(), "resilience".into()],
            configuration: IndexMap::new(),
            secrets: IndexMap::new(),
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

    // ── ProbeBaseline ───────────────────────────────────────────

    #[test]
    fn probe_baseline_round_trips() {
        let pb = ProbeBaseline {
            name: "api-latency".into(),
            mean: 45.2,
            stddev: 8.3,
            p50: 43.1,
            p95: 58.7,
            p99: 72.4,
            min: 12.0,
            max: 98.3,
            error_rate: 0.001,
            samples: 60,
        };
        let decoded: ProbeBaseline = toon_round_trip(&pb);
        assert_eq!(decoded, pb);
    }

    // ── BaselineResult ─────────────────────────────────────────

    #[test]
    fn baseline_result_round_trips() {
        let result = BaselineResult {
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_120_000_000_000,
            duration_s: 120.0,
            warmup_s: 15.0,
            samples: 60,
            interval_s: 2.0,
            method: BaselineMethod::MeanStddev,
            sigma: Some(2.0),
            source: BaselineSource::Live,
            anomaly_detected: false,
            probes: vec![ProbeBaseline {
                name: "api-latency".into(),
                mean: 45.2,
                stddev: 8.3,
                p50: 43.1,
                p95: 58.7,
                p99: 72.4,
                min: 12.0,
                max: 98.3,
                error_rate: 0.001,
                samples: 60,
            }],
            tolerance_lower: 28.6,
            tolerance_upper: 61.8,
        };
        let decoded: BaselineResult = toon_round_trip(&result);
        assert_eq!(decoded, result);
    }

    // ── ProbeDuring ────────────────────────────────────────────

    #[test]
    fn probe_during_round_trips() {
        let pd = ProbeDuring {
            name: "api-latency".into(),
            samples: 30,
            mean: 342.8,
            max: 1204.3,
            min: 45.0,
            error_rate: 0.12,
            breached_at_ns: Some(1_774_980_136_000_000_000),
            breach_count: 18,
        };
        let decoded: ProbeDuring = toon_round_trip(&pd);
        assert_eq!(decoded, pd);
    }

    // ── DuringResult ───────────────────────────────────────────

    #[test]
    fn during_result_round_trips() {
        let result = DuringResult {
            started_at_ns: 1_774_980_135_000_000_000,
            ended_at_ns: 1_774_980_165_000_000_000,
            fault_active_s: 30.0,
            sample_interval_s: 1.0,
            probes: vec![ProbeDuring {
                name: "api-latency".into(),
                samples: 30,
                mean: 342.8,
                max: 1204.3,
                min: 45.0,
                error_rate: 0.12,
                breached_at_ns: Some(1_774_980_136_000_000_000),
                breach_count: 18,
            }],
            degradation_onset_s: Some(1.0),
            degradation_peak_s: Some(8.3),
            degradation_magnitude: Some(35.8),
            graceful_degradation: Some(true),
        };
        let decoded: DuringResult = toon_round_trip(&result);
        assert_eq!(decoded, result);
    }

    // ── ProbePost ──────────────────────────────────────────────

    #[test]
    fn probe_post_round_trips() {
        let pp = ProbePost {
            name: "api-latency".into(),
            mean: 46.1,
            p95: 59.2,
            error_rate: 0.002,
            returned_to_baseline: true,
            recovery_time_s: 12.4,
        };
        let decoded: ProbePost = toon_round_trip(&pp);
        assert_eq!(decoded, pp);
    }

    // ── PostResult ─────────────────────────────────────────────

    #[test]
    fn post_result_round_trips() {
        let result = PostResult {
            started_at_ns: 1_774_980_165_000_000_000,
            ended_at_ns: 1_774_980_285_000_000_000,
            duration_s: 120.0,
            samples: 60,
            probes: vec![ProbePost {
                name: "api-latency".into(),
                mean: 46.1,
                p95: 59.2,
                error_rate: 0.002,
                returned_to_baseline: true,
                recovery_time_s: 12.4,
            }],
            recovery_time_s: 12.4,
            full_recovery: true,
            residual_degradation: Some(0.1),
            data_integrity_verified: Some(true),
            data_loss_detected: Some(false),
            mttr_s: None,
        };
        let decoded: PostResult = toon_round_trip(&result);
        assert_eq!(decoded, result);
    }

    // ── LoadResult ─────────────────────────────────────────────

    #[test]
    fn load_result_round_trips() {
        let result = LoadResult {
            tool: LoadTool::K6,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_300_000_000_000,
            duration_s: 300.0,
            vus: 50,
            throughput_rps: 494.1,
            latency_p50_ms: 42.3,
            latency_p95_ms: 187.4,
            latency_p99_ms: 342.1,
            error_rate: 0.008,
            total_requests: 148_230,
            thresholds_met: true,
        };
        let decoded: LoadResult = toon_round_trip(&result);
        assert_eq!(decoded, result);
    }

    // ── AnalysisResult ─────────────────────────────────────────

    #[test]
    fn analysis_result_round_trips() {
        let result = AnalysisResult {
            estimate_accuracy: Some(0.83),
            estimate_recovery_delta_s: Some(-2.6),
            trend: Some(Trend::Improving),
            resilience_score: Some(0.92),
        };
        let decoded: AnalysisResult = toon_round_trip(&result);
        assert_eq!(decoded, result);
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

    // ── Journal (updated with all phases) ──────────────────────

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
        };
        let decoded: Journal = toon_round_trip(&journal);
        assert_eq!(decoded, journal);
    }

    // ── Journal ────────────────────────────────────────────────

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
        };
        let decoded: Journal = toon_round_trip(&journal);
        assert_eq!(decoded, journal);
    }
}
