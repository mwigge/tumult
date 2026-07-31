//! Serde round-trip tests for the baseline data-model types: every config
//! and result type must survive serialize → deserialize unchanged, since
//! these cross process and storage boundaries.

use tumult_baseline::tolerance::Method;
use tumult_baseline::{
    AcquisitionConfig, AcquisitionResult, AnomalyCheck, BaselineBounds, ProbeSamples, ProbeStats,
};

fn round_trip<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).unwrap();
    let back: T = serde_json::from_str(&json).unwrap();
    assert_eq!(*value, back);
}

#[test]
fn anomaly_check_round_trips() {
    round_trip(&AnomalyCheck {
        anomaly_detected: true,
        reason: Some("high variance: coefficient of variation 0.75 exceeds 0.50".into()),
        coefficient_of_variation: 0.75,
    });
    round_trip(&AnomalyCheck {
        anomaly_detected: false,
        reason: None,
        coefficient_of_variation: 0.0,
    });
}

#[test]
fn baseline_bounds_round_trips() {
    round_trip(&BaselineBounds {
        lower: 42.5,
        upper: 157.5,
    });
}

#[test]
fn method_round_trips() {
    round_trip(&Method::Static {
        lower: 10.0,
        upper: 90.0,
    });
    round_trip(&Method::Percentile {
        percentile: 95.0,
        multiplier: 1.2,
    });
    round_trip(&Method::MeanStddev { sigma: 2.0 });
    round_trip(&Method::Iqr);
}

#[test]
fn probe_stats_round_trips() {
    round_trip(&ProbeStats {
        name: "api-latency".into(),
        mean: 100.0,
        stddev: 2.5,
        p50: 100.0,
        p95: 103.0,
        p99: 104.0,
        min: 97.0,
        max: 105.0,
        error_rate: 0.02,
        samples: 50,
        tolerance_lower: 95.0,
        tolerance_upper: 105.0,
    });
}

#[test]
fn acquisition_result_round_trips() {
    round_trip(&AcquisitionResult {
        probes: vec![ProbeStats {
            name: "api-latency".into(),
            mean: 100.0,
            stddev: 2.5,
            p50: 100.0,
            p95: 103.0,
            p99: 104.0,
            min: 97.0,
            max: 105.0,
            error_rate: 0.0,
            samples: 50,
            tolerance_lower: 95.0,
            tolerance_upper: 105.0,
        }],
        tolerance_lower: 95.0,
        tolerance_upper: 105.0,
        anomaly_detected: false,
        anomaly_reason: None,
        total_samples: 50,
    });
}

#[test]
fn acquisition_config_round_trips() {
    round_trip(&AcquisitionConfig {
        method: Method::MeanStddev { sigma: 2.0 },
        min_samples: 5,
    });
}

#[test]
fn probe_samples_round_trips() {
    round_trip(&ProbeSamples {
        name: "api-latency".into(),
        values: vec![100.0, 102.0, 98.0, 101.0],
        errors: 1,
        total_attempts: 5,
        sampled_at: vec![1_700_000_000_000_000_000, 1_700_000_001_000_000_000],
    });
}
