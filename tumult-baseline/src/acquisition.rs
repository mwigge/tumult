//! Baseline acquisition — orchestrates warmup, sampling, and derivation.
//!
//! The acquisition module takes pre-collected probe samples and produces
//! a complete `AcquisitionResult` with per-probe statistics, tolerance
//! bounds, and anomaly detection.
//!
//! This module is intentionally synchronous. The async probe execution
//! loop lives in `tumult-core`'s runner; this module consumes the
//! collected samples and derives the baseline.

use crate::anomaly::check_baseline_anomaly;
use crate::stats::{mean, stddev};
use crate::tolerance::{derive_tolerance, Method};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AcquisitionError {
    #[error("no probes provided")]
    NoProbes,
    #[error("probe '{name}' has no samples after warmup")]
    NoSamplesAfterWarmup { name: String },
}

/// Per-probe statistics derived from baseline samples.
#[derive(Debug, Clone)]
pub struct ProbeStats {
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

/// Result of a complete baseline acquisition.
#[derive(Debug, Clone)]
pub struct AcquisitionResult {
    pub probes: Vec<ProbeStats>,
    pub tolerance_lower: f64,
    pub tolerance_upper: f64,
    pub anomaly_detected: bool,
    pub anomaly_reason: Option<String>,
    pub total_samples: u32,
}

/// Configuration for baseline acquisition.
#[derive(Debug, Clone)]
pub struct AcquisitionConfig {
    pub method: Method,
    /// Minimum number of samples required before declaring anomaly.
    pub min_samples: usize,
}

/// Samples collected for a single probe during baseline.
#[derive(Debug, Clone)]
pub struct ProbeSamples {
    pub name: String,
    /// Numeric values collected (e.g., response time in ms).
    pub values: Vec<f64>,
    /// Number of errors observed during sampling.
    pub errors: u32,
    /// Total attempts (successful + failed).
    pub total_attempts: u32,
    /// Epoch nanosecond timestamps for each sample in `values`.
    ///
    /// Used for Arrow conversion and MTTR analysis. May be empty if
    /// the caller does not track per-sample timestamps.
    pub sampled_at: Vec<i64>,
}

/// Streaming baseline acquisition builder.
///
/// Accepts probe samples incrementally — one value at a time — and
/// derives the final baseline when [`finish`] is called.
///
/// This is a synchronous, allocation-friendly alternative to building a
/// complete [`ProbeSamples`] vector before calling [`derive_baseline`].
/// The async probe loop pushes each result here as it arrives; the runner
/// calls [`finish`] at the end of the warmup window.
///
/// # Examples
///
/// ```
/// use tumult_baseline::acquisition::{AcquisitionStream, AcquisitionConfig};
/// use tumult_baseline::tolerance::Method;
///
/// let mut stream = AcquisitionStream::new(
///     "api-latency".into(),
///     AcquisitionConfig {
///         method: Method::MeanStddev { sigma: 2.0 },
///         min_samples: 3,
///     },
/// );
///
/// stream.push_sample(100.0);
/// stream.push_sample(102.0);
/// stream.push_sample(98.0);
///
/// let result = stream.finish().unwrap();
/// assert_eq!(result.probes.len(), 1);
/// assert!(!result.anomaly_detected);
/// ```
pub struct AcquisitionStream {
    probe_name: String,
    config: AcquisitionConfig,
    values: Vec<f64>,
    errors: u32,
    total_attempts: u32,
}

impl AcquisitionStream {
    /// Creates a new streaming acquisition for a single probe.
    #[must_use]
    pub fn new(probe_name: String, config: AcquisitionConfig) -> Self {
        Self {
            probe_name,
            config,
            values: Vec::new(),
            errors: 0,
            total_attempts: 0,
        }
    }

    /// Records a successful probe sample value.
    pub fn push_sample(&mut self, value: f64) {
        self.values.push(value);
        self.total_attempts += 1;
    }

    /// Records a probe error (no value collected).
    pub fn push_error(&mut self) {
        self.errors += 1;
        self.total_attempts += 1;
    }

    /// Returns the number of successful samples pushed so far.
    #[must_use]
    pub fn sample_count(&self) -> usize {
        self.values.len()
    }

    /// Derives the baseline from all pushed samples.
    ///
    /// Equivalent to calling [`derive_baseline`] with the accumulated
    /// [`ProbeSamples`]. Does not consume the stream — samples can continue
    /// to be pushed after calling `derive`.
    ///
    /// # Errors
    ///
    /// Returns [`AcquisitionError::NoSamplesAfterWarmup`] if no successful
    /// samples have been pushed.
    pub fn derive(&self) -> Result<AcquisitionResult, AcquisitionError> {
        let probe = ProbeSamples {
            name: self.probe_name.clone(),
            values: self.values.clone(),
            errors: self.errors,
            total_attempts: self.total_attempts,
            sampled_at: vec![],
        };
        derive_baseline(&[probe], &self.config)
    }

    /// Finalises the stream and derives the baseline, consuming `self`.
    ///
    /// # Errors
    ///
    /// Returns [`AcquisitionError::NoSamplesAfterWarmup`] if no successful
    /// samples were pushed.
    pub fn finish(self) -> Result<AcquisitionResult, AcquisitionError> {
        let probe = ProbeSamples {
            name: self.probe_name,
            values: self.values,
            errors: self.errors,
            total_attempts: self.total_attempts,
            sampled_at: vec![],
        };
        derive_baseline(&[probe], &self.config)
    }
}

fn percentile_sorted(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let p = p.clamp(0.0, 100.0);
    // Percentile rank computation: lengths are at most a few thousand elements,
    // so precision loss from usize->f64 and sign/truncation from f64->usize are acceptable.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let rank = (p / 100.0) * (sorted.len() - 1) as f64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lower = rank.floor() as usize;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let upper = rank.ceil() as usize;
    #[allow(clippy::cast_precision_loss)]
    let fraction = rank - lower as f64;
    sorted[lower] + fraction * (sorted[upper] - sorted[lower])
}

/// Derive baseline statistics from pre-collected probe samples.
///
/// The caller is responsible for:
/// 1. Executing probes at the configured interval
/// 2. Discarding warmup samples
/// 3. Collecting successful values and error counts
///
/// This function computes statistics, checks for anomalies, and derives
/// tolerance bounds from the samples.
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
    let mut all_values: Vec<f64> = Vec::new();
    let mut total_samples: u32 = 0;
    let mut any_anomaly = false;
    let mut anomaly_reason = None;

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

        let stats = ProbeStats {
            name: ps.name.clone(),
            mean: mean(&ps.values),
            stddev: stddev(&ps.values),
            p50: percentile_sorted(&sorted, 50.0),
            p95: percentile_sorted(&sorted, 95.0),
            p99: percentile_sorted(&sorted, 99.0),
            min: sorted[0],
            max: sorted[sorted.len() - 1],
            error_rate,
            samples: sample_count,
        };

        total_samples = total_samples.saturating_add(stats.samples);
        all_values.extend_from_slice(&ps.values);
        probes.push(stats);
    }

    // Check for anomalies across all combined samples
    let anomaly_check = check_baseline_anomaly(&all_values, config.min_samples);
    if anomaly_check.anomaly_detected {
        any_anomaly = true;
        anomaly_reason = anomaly_check.reason;
    }

    // Derive tolerance bounds from all combined samples
    let bounds = derive_tolerance(&all_values, &config.method);

    if any_anomaly {
        if let Some(ref reason) = anomaly_reason {
            let cv = stddev(&all_values) / mean(&all_values);
            crate::telemetry::event_anomaly_detected(reason, cv);
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

    // ── AcquisitionStream ─────────────────────────────────────

    #[test]
    fn acquisition_stream_finish_derives_baseline() {
        let mut stream = AcquisitionStream::new("latency".into(), config_mean_stddev());
        for v in [100.0, 102.0, 98.0, 101.0, 99.0] {
            stream.push_sample(v);
        }
        let result = stream.finish().unwrap();
        assert_eq!(result.probes.len(), 1);
        assert_eq!(result.probes[0].name, "latency");
        assert!((result.probes[0].mean - 100.0).abs() < 1.0);
        assert!(!result.anomaly_detected);
    }

    #[test]
    fn acquisition_stream_push_error_tracks_error_rate() {
        let mut stream = AcquisitionStream::new("check".into(), config_mean_stddev());
        for v in [100.0, 101.0, 99.0, 100.0, 102.0] {
            stream.push_sample(v);
        }
        stream.push_error();
        stream.push_error();
        let result = stream.finish().unwrap();
        let expected_rate = 2.0 / 7.0;
        assert!((result.probes[0].error_rate - expected_rate).abs() < 0.001);
    }

    #[test]
    fn acquisition_stream_derive_does_not_consume() {
        let mut stream = AcquisitionStream::new("latency".into(), config_mean_stddev());
        for v in [100.0, 102.0, 98.0, 101.0, 99.0] {
            stream.push_sample(v);
        }
        // derive() borrows; can push more after
        let mid_result = stream.derive().unwrap();
        assert_eq!(mid_result.probes[0].samples, 5);
        stream.push_sample(103.0);
        assert_eq!(stream.sample_count(), 6);
        let final_result = stream.finish().unwrap();
        assert_eq!(final_result.probes[0].samples, 6);
    }
}
