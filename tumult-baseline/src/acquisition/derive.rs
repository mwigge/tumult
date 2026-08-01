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
mod tests;
