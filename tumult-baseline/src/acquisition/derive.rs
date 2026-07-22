//! Core derivation: percentile computation and baseline statistics.

use super::types::{
    AcquisitionConfig, AcquisitionError, AcquisitionResult, ProbeSamples, ProbeStats,
};

use crate::anomaly::check_baseline_anomaly;
use crate::stats::{mean, percentile_sorted, stddev, BaselineBounds};
use crate::tolerance::derive_tolerance;

/// Derive baseline statistics from pre-collected probe samples.
///
/// The caller is responsible for:
/// 1. Executing probes at the configured interval
/// 2. Discarding warmup samples
/// 3. Collecting successful values and error counts
///
/// All statistics, anomaly checks, and tolerance bounds are computed **per
/// probe**. Probes measure different quantities on different scales (a ~100ms
/// latency probe vs a ~5000 rps throughput probe); pooling their samples into
/// one distribution produces a meaningless coefficient of variation and false
/// "high variance" anomalies. The headline `tolerance_lower`/`tolerance_upper`
/// on the result are the bounds of the worst-CV probe — see
/// [`AcquisitionResult`].
///
/// # Errors
///
/// Returns [`AcquisitionError::NoProbes`] if `probe_samples` is empty.
/// Returns [`AcquisitionError::NoSamplesAfterWarmup`] if any probe has no
/// collected values.
///
/// # Examples
///
/// ```
/// use tumult_baseline::{
///     derive_baseline, AcquisitionConfig, ProbeSamples,
/// };
/// use tumult_baseline::tolerance::Method;
///
/// let samples = vec![ProbeSamples {
///     name: "api-latency".into(),
///     values: vec![100.0, 102.0, 98.0, 101.0, 99.0, 100.0, 103.0, 97.0],
///     errors: 0,
///     total_attempts: 8,
///     sampled_at: vec![],
/// }];
///
/// let config = AcquisitionConfig {
///     method: Method::MeanStddev { sigma: 2.0 },
///     min_samples: 5,
/// };
///
/// let result = derive_baseline(&samples, &config).unwrap();
/// assert_eq!(result.probes.len(), 1);
/// assert!(!result.anomaly_detected);
/// assert!(result.tolerance_lower < 100.0);
/// assert!(result.tolerance_upper > 100.0);
/// ```
pub fn derive_baseline(
    probe_samples: &[ProbeSamples],
    config: &AcquisitionConfig,
) -> Result<AcquisitionResult, AcquisitionError> {
    let method_name = match &config.method {
        crate::tolerance::Method::MeanStddev { .. } => "mean_stddev",
        crate::tolerance::Method::Iqr => "iqr",
        crate::tolerance::Method::Percentile { .. } => "percentile",
        crate::tolerance::Method::Static { .. } => "static",
    };
    let _span = crate::telemetry::begin_acquire(probe_samples.len(), method_name);

    if probe_samples.is_empty() {
        return Err(AcquisitionError::NoProbes);
    }

    let mut probes = Vec::with_capacity(probe_samples.len());
    let mut total_samples: u32 = 0;
    let mut any_anomaly = false;
    let mut anomaly_reason = None;
    // Worst-case (highest) per-probe CV and the bounds of the probe that
    // produced it. Re-used directly for telemetry — never recomputed via a
    // second stddev()/mean() pass (BAS-MED-1).
    let mut worst_cv = 0.0_f64;
    let mut worst_cv_bounds: Option<BaselineBounds> = None;

    for ps in probe_samples {
        if ps.values.is_empty() {
            return Err(AcquisitionError::NoSamplesAfterWarmup {
                name: ps.name.clone(),
            });
        }

        let error_rate = if ps.total_attempts > 0 {
            f64::from(ps.errors) / f64::from(ps.total_attempts)
        } else {
            0.0
        };

        // Sort once, compute all percentiles from sorted slice
        let mut sorted = ps.values.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let sample_count = u32::try_from(ps.values.len()).unwrap_or(u32::MAX);

        // Per-probe tolerance bounds from this probe's own samples.
        let probe_bounds = derive_tolerance(&ps.values, &config.method);

        let stats = ProbeStats {
            name: ps.name.clone(),
            mean: mean(&ps.values).unwrap_or(0.0),
            // N < 2 has no defined sample stddev; report 0 spread.
            stddev: stddev(&ps.values).unwrap_or(0.0),
            p50: percentile_sorted(&sorted, 50.0),
            p95: percentile_sorted(&sorted, 95.0),
            p99: percentile_sorted(&sorted, 99.0),
            min: sorted[0],
            max: sorted[sorted.len() - 1],
            error_rate,
            samples: sample_count,
            tolerance_lower: probe_bounds.lower,
            tolerance_upper: probe_bounds.upper,
        };

        total_samples = total_samples.saturating_add(stats.samples);

        // Per-probe anomaly check — pooling across probes would compare
        // values on incompatible scales.
        let check = check_baseline_anomaly(&ps.values, config.min_samples);
        if check.anomaly_detected && !any_anomaly {
            any_anomaly = true;
            anomaly_reason = check
                .reason
                .map(|reason| format!("probe '{}': {reason}", ps.name));
        }
        if worst_cv_bounds.is_none() || check.coefficient_of_variation > worst_cv {
            worst_cv = check.coefficient_of_variation;
            worst_cv_bounds = Some(probe_bounds);
        }

        probes.push(stats);
    }

    // A single probe keeps the historical headline bounds exactly; multiple
    // probes report the noisiest (worst-CV) probe's bounds. `worst_cv_bounds`
    // is always `Some` here because the loop ran at least once.
    let bounds = worst_cv_bounds.unwrap_or(BaselineBounds {
        lower: 0.0,
        upper: 0.0,
    });

    if any_anomaly {
        if let Some(ref reason) = anomaly_reason {
            crate::telemetry::event_anomaly_detected(reason, worst_cv);
        }
    }

    crate::telemetry::event_tolerance_derived(bounds.lower, bounds.upper, total_samples as usize);
    crate::telemetry::record_baseline_gauges(
        probes.len(),
        total_samples as usize,
        bounds.lower,
        bounds.upper,
    );

    Ok(AcquisitionResult {
        probes,
        tolerance_lower: bounds.lower,
        tolerance_upper: bounds.upper,
        anomaly_detected: any_anomaly,
        anomaly_reason,
        total_samples,
    })
}

#[cfg(test)]
mod tests {
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
        assert!(result.is_err());
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
        assert!(result.is_err());
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

    /// Verify that the CV passed to the anomaly telemetry event matches what
    /// `check_baseline_anomaly` computes — i.e., it is not recomputed from
    /// scratch via a second `stddev/mean` call (BAS-MED-1).
    #[test]
    fn anomaly_cv_is_not_recomputed() {
        use crate::anomaly::check_baseline_anomaly;

        let values = vec![1.0, 100.0, 2.0, 99.0, 3.0, 98.0, 1.0, 200.0];
        // The CV stored on AnomalyCheck is the authoritative value.
        let anomaly_check = check_baseline_anomaly(&values, 5);
        assert!(anomaly_check.anomaly_detected);
        // CV must be nonzero for high-variance data.
        assert!(
            anomaly_check.coefficient_of_variation > 0.0,
            "expected nonzero CV, got {}",
            anomaly_check.coefficient_of_variation
        );
        // derive_baseline must succeed without recomputing CV.
        let samples = vec![ProbeSamples {
            name: "unstable2".into(),
            values,
            errors: 0,
            total_attempts: 8,
            sampled_at: vec![],
        }];
        let result = derive_baseline(&samples, &config_mean_stddev()).unwrap();
        assert!(result.anomaly_detected);
    }

    #[test]
    fn iqr_method_works() {
        let samples = vec![stable_samples("latency")];
        let config = AcquisitionConfig {
            method: Method::Iqr,
            min_samples: 5,
        };
        let result = derive_baseline(&samples, &config).unwrap();
        assert!(!result.anomaly_detected);
        // IQR bounds should be wider than data range for stable data
        assert!(result.tolerance_lower < 97.0);
        assert!(result.tolerance_upper > 103.0);
    }

    #[test]
    fn percentile_method_works() {
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
        assert!(result.tolerance_upper > 100.0);
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
}
