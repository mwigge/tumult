//! Per-phase measurement result types (baseline, during, post, load, analysis).

use serde::{Deserialize, Serialize};

use super::enums::{BaselineMethod, BaselineSource, LoadTool, Trend};

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

#[cfg(test)]
mod tests {
    use crate::types::test_support::toon_round_trip;
    use crate::types::*;

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
}
