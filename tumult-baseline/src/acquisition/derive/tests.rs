use super::*;
use crate::tolerance::Method;

fn stable_samples(name: &str) -> ProbeSamples {
    ProbeSamples {
        name: name.into(),
        values: vec![
            100.0, 102.0, 98.0, 101.0, 99.0, 100.0, 103.0, 97.0, 101.0, 99.0,
        ],
        errors: 0,
        total_attempts: 10,
        sampled_at: vec![],
    }
}

fn config_mean_stddev() -> AcquisitionConfig {
    AcquisitionConfig {
        method: Method::MeanStddev { sigma: 2.0 },
        min_samples: 5,
    }
}

// ── derive_baseline ───────────────────────────────────────

#[test]
fn single_probe_derives_baseline() {
    let samples = vec![stable_samples("api-latency")];
    let result = derive_baseline(&samples, &config_mean_stddev()).unwrap();

    assert_eq!(result.probes.len(), 1);
    assert_eq!(result.probes[0].name, "api-latency");
    assert_eq!(result.probes[0].samples, 10);
    assert!((result.probes[0].mean - 100.0).abs() < 1.0);
    assert!(!result.anomaly_detected);
    assert!(result.tolerance_lower < 100.0);
    assert!(result.tolerance_upper > 100.0);
}

#[test]
fn multiple_probes_derives_baseline() {
    let samples = vec![stable_samples("latency"), stable_samples("throughput")];
    let result = derive_baseline(&samples, &config_mean_stddev()).unwrap();

    assert_eq!(result.probes.len(), 2);
    assert_eq!(result.total_samples, 20);
}

/// The headline defect: probes on incompatible scales (latency ~100ms,
/// throughput ~5000 rps) pooled into one distribution produced a huge
/// pooled CV and a false "high variance" anomaly. Per-probe statistics
/// must see both as stable.
#[test]
fn multi_probe_different_scales_no_false_anomaly() {
    let latency = ProbeSamples {
        name: "latency".into(),
        values: vec![
            100.0, 102.0, 98.0, 101.0, 99.0, 100.0, 103.0, 97.0, 101.0, 99.0,
        ],
        errors: 0,
        total_attempts: 10,
        sampled_at: vec![],
    };
    let throughput = ProbeSamples {
        name: "throughput".into(),
        values: vec![
            5000.0, 5050.0, 4950.0, 5025.0, 4975.0, 5010.0, 4990.0, 5030.0, 4980.0, 5020.0,
        ],
        errors: 0,
        total_attempts: 10,
        sampled_at: vec![],
    };
    let result = derive_baseline(&[latency, throughput], &config_mean_stddev()).unwrap();
    assert!(
        !result.anomaly_detected,
        "two stable probes on different scales must not be flagged anomalous"
    );

    let lat = &result.probes[0];
    let thr = &result.probes[1];
    // Each probe's bounds sit on its own scale.
    assert!(lat.tolerance_lower < 100.0 && lat.tolerance_upper > 100.0);
    assert!(
        lat.tolerance_upper < 1000.0,
        "latency bounds must not absorb the throughput scale: {}",
        lat.tolerance_upper
    );
    assert!(thr.tolerance_lower > 1000.0 && thr.tolerance_lower < 5000.0);
    assert!(thr.tolerance_upper > 5000.0);
}

#[test]
fn multi_probe_anomaly_reason_names_the_offending_probe() {
    let stable = stable_samples("stable");
    let noisy = ProbeSamples {
        name: "noisy".into(),
        values: vec![1.0, 100.0, 2.0, 99.0, 3.0, 98.0, 1.0, 200.0],
        errors: 0,
        total_attempts: 8,
        sampled_at: vec![],
    };
    let result = derive_baseline(&[stable, noisy], &config_mean_stddev()).unwrap();
    assert!(result.anomaly_detected);
    let reason = result.anomaly_reason.unwrap();
    assert!(
        reason.contains("noisy"),
        "reason must identify the anomalous probe: {reason}"
    );
}

#[test]
fn single_sample_probe_derives_zero_stddev_and_collapsed_bounds() {
    let samples = vec![ProbeSamples {
        name: "single".into(),
        values: vec![42.0],
        errors: 0,
        total_attempts: 1,
        sampled_at: vec![],
    }];
    let config = AcquisitionConfig {
        method: Method::MeanStddev { sigma: 2.0 },
        min_samples: 1,
    };
    let result = derive_baseline(&samples, &config).unwrap();
    assert!((result.probes[0].mean - 42.0).abs() < f64::EPSILON);
    // Sample stddev is undefined for N = 1; reported spread is 0.
    assert!(result.probes[0].stddev.abs() < f64::EPSILON);
    assert!((result.probes[0].tolerance_lower - 42.0).abs() < f64::EPSILON);
    assert!((result.probes[0].tolerance_upper - 42.0).abs() < f64::EPSILON);
    assert!(!result.anomaly_detected);
}

#[test]
fn error_rate_computed_correctly() {
    let samples = vec![ProbeSamples {
        name: "check".into(),
        values: vec![100.0, 101.0, 99.0, 100.0, 102.0],
        errors: 2,
        total_attempts: 7,
        sampled_at: vec![],
    }];
    let result = derive_baseline(&samples, &config_mean_stddev()).unwrap();
    let expected_rate = 2.0 / 7.0;
    assert!((result.probes[0].error_rate - expected_rate).abs() < 0.001);
}

#[test]
fn empty_probes_returns_error() {
    let result = derive_baseline(&[], &config_mean_stddev());
    assert!(
        matches!(result, Err(AcquisitionError::NoProbes)),
        "{result:?}"
    );
}

#[test]
fn empty_values_returns_error() {
    let samples = vec![ProbeSamples {
        name: "empty".into(),
        values: vec![],
        errors: 0,
        total_attempts: 0,
        sampled_at: vec![],
    }];
    let result = derive_baseline(&samples, &config_mean_stddev());
    assert!(
        matches!(&result, Err(AcquisitionError::NoSamplesAfterWarmup { name }) if name == "empty"),
        "{result:?}"
    );
}

#[test]
fn high_variance_detects_anomaly() {
    let samples = vec![ProbeSamples {
        name: "unstable".into(),
        values: vec![1.0, 100.0, 2.0, 99.0, 3.0, 98.0, 1.0, 200.0],
        errors: 0,
        total_attempts: 8,
        sampled_at: vec![],
    }];
    let result = derive_baseline(&samples, &config_mean_stddev()).unwrap();
    assert!(result.anomaly_detected);
    assert!(result.anomaly_reason.is_some());
}

/// The anomaly event must carry the CV `check_baseline_anomaly` computed for
/// the anomalous probe, and the span must describe the acquisition
/// (BAS-MED-1: the CV is reused, not recomputed over pooled samples).
#[test]
fn anomaly_event_carries_the_cv_of_the_anomalous_probe() {
    use crate::anomaly::check_baseline_anomaly;
    use opentelemetry::trace::Status;
    use opentelemetry::{KeyValue, Value};
    use tumult_test_utils::{attribute, capture_spans};

    let unstable = vec![1.0, 100.0, 2.0, 99.0, 3.0, 98.0, 1.0, 200.0];
    let expected_cv = check_baseline_anomaly(&unstable, 5).coefficient_of_variation;
    let samples = vec![
        stable_samples("steady"),
        ProbeSamples {
            name: "unstable".into(),
            values: unstable,
            errors: 0,
            total_attempts: 8,
            sampled_at: vec![],
        },
    ];

    let (result, spans) = capture_spans(|| derive_baseline(&samples, &config_mean_stddev()));
    let result = result.unwrap();

    assert_eq!(spans.len(), 1);
    let span = &spans[0];
    assert_eq!(span.name, "baseline.acquire");
    assert_eq!(
        span.attributes,
        [
            KeyValue::new("baseline.probe_count", 2_i64),
            KeyValue::new("baseline.tolerance_method", "mean_stddev"),
        ]
    );
    // Mean 63, sample standard deviation 73.36: CV 1.16.
    let reason = "probe 'unstable': high variance: coefficient of variation 1.16 exceeds 0.50";
    assert_eq!(span.status, Status::error(reason));

    let events = &span.events.events;
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].name, "anomaly.detected");
    assert_eq!(
        events[0].attributes,
        [
            KeyValue::new("baseline.anomaly.reason", reason),
            KeyValue::new("baseline.anomaly.cv", expected_cv),
        ]
    );
    // The headline bounds on the event are those of the returned result.
    assert_eq!(events[1].name, "tolerance.derived");
    assert_eq!(
        attribute(&events[1].attributes, "baseline.tolerance.lower"),
        Some(&Value::F64(result.tolerance_lower))
    );
    assert_eq!(
        attribute(&events[1].attributes, "baseline.tolerance.upper"),
        Some(&Value::F64(result.tolerance_upper))
    );
    assert_eq!(
        attribute(&events[1].attributes, "baseline.samples_total"),
        Some(&Value::I64(18))
    );
}

#[test]
fn iqr_method_derives_quartile_bounds() {
    let samples = vec![stable_samples("latency")];
    let config = AcquisitionConfig {
        method: Method::Iqr,
        min_samples: 5,
    };
    let result = derive_baseline(&samples, &config).unwrap();
    assert!(!result.anomaly_detected);
    // Q1 = 99, Q3 = 101, IQR = 2: bounds are 99 - 3 and 101 + 3.
    assert!((result.tolerance_lower - 96.0).abs() < 1e-9, "{result:?}");
    assert!((result.tolerance_upper - 104.0).abs() < 1e-9, "{result:?}");
}

#[test]
fn percentile_method_derives_scaled_upper_bound() {
    let samples = vec![stable_samples("latency")];
    let config = AcquisitionConfig {
        method: Method::Percentile {
            percentile: 95.0,
            multiplier: 1.2,
        },
        min_samples: 5,
    };
    let result = derive_baseline(&samples, &config).unwrap();
    assert!(!result.anomaly_detected);
    assert!(result.tolerance_lower.abs() < f64::EPSILON);
    // p95 = 102.55 (rank 8.55 between 102 and 103), times 1.2.
    assert!((result.tolerance_upper - 123.06).abs() < 1e-9, "{result:?}");
}

#[test]
fn static_method_ignores_data() {
    let samples = vec![stable_samples("latency")];
    let config = AcquisitionConfig {
        method: Method::Static {
            lower: 50.0,
            upper: 150.0,
        },
        min_samples: 5,
    };
    let result = derive_baseline(&samples, &config).unwrap();
    assert!((result.tolerance_lower - 50.0).abs() < f64::EPSILON);
    assert!((result.tolerance_upper - 150.0).abs() < f64::EPSILON);
}

#[test]
fn percentile_sorted_matches_expected() {
    let sorted: Vec<f64> = (1..=100).map(f64::from).collect();
    assert!((percentile_sorted(&sorted, 0.0) - 1.0).abs() < f64::EPSILON);
    assert!((percentile_sorted(&sorted, 100.0) - 100.0).abs() < f64::EPSILON);
    assert!((percentile_sorted(&sorted, 50.0) - 50.5).abs() < 1.0);
}

#[test]
fn percentile_sorted_empty_returns_zero() {
    assert!((percentile_sorted(&[], 50.0)).abs() < f64::EPSILON);
}

#[test]
fn percentile_sorted_single_returns_value() {
    assert!((percentile_sorted(&[42.0], 95.0) - 42.0).abs() < f64::EPSILON);
}

#[test]
fn min_max_computed_directly() {
    let samples = vec![ProbeSamples {
        name: "check".into(),
        values: vec![50.0, 10.0, 90.0, 30.0, 70.0],
        errors: 0,
        total_attempts: 5,
        sampled_at: vec![],
    }];
    let result = derive_baseline(&samples, &config_mean_stddev()).unwrap();
    assert!((result.probes[0].min - 10.0).abs() < f64::EPSILON);
    assert!((result.probes[0].max - 90.0).abs() < f64::EPSILON);
}

#[test]
fn probe_stats_has_correct_percentiles() {
    let samples = vec![ProbeSamples {
        name: "ordered".into(),
        values: (1..=100).map(f64::from).collect(),
        errors: 0,
        total_attempts: 100,
        sampled_at: vec![],
    }];
    let result = derive_baseline(&samples, &config_mean_stddev()).unwrap();
    let stats = &result.probes[0];

    assert!((stats.min - 1.0).abs() < f64::EPSILON);
    assert!((stats.max - 100.0).abs() < f64::EPSILON);
    assert!((stats.p50 - 50.5).abs() < 1.0);
    assert!(stats.p95 > 90.0);
    assert!(stats.p99 > 95.0);
}
