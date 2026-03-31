//! Tolerance derivation from baseline data.
//!
//! Given a set of baseline samples and a method configuration,
//! derive the upper and lower tolerance bounds for steady-state
//! hypothesis evaluation.

use crate::stats::{
    derive_iqr_bounds, derive_mean_stddev_bounds, derive_percentile, BaselineBounds,
};

/// Supported baseline methods.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum Method {
    /// Fixed threshold — no derivation needed.
    Static { lower: f64, upper: f64 },
    /// Percentile-based: threshold = p(N) * multiplier.
    Percentile { percentile: f64, multiplier: f64 },
    /// Mean ± N standard deviations.
    MeanStddev { sigma: f64 },
    /// Interquartile range: Q1 - 1.5*IQR to Q3 + 1.5*IQR.
    Iqr,
}

/// Derive tolerance bounds from baseline samples using the specified method.
#[must_use]
pub fn derive_tolerance(samples: &[f64], method: &Method) -> BaselineBounds {
    match method {
        Method::Static { lower, upper } => BaselineBounds {
            lower: *lower,
            upper: *upper,
        },
        Method::Percentile {
            percentile,
            multiplier,
        } => {
            let threshold = derive_percentile(samples, *percentile, *multiplier);
            BaselineBounds {
                lower: 0.0,
                upper: threshold,
            }
        }
        Method::MeanStddev { sigma } => derive_mean_stddev_bounds(samples, *sigma),
        Method::Iqr => derive_iqr_bounds(samples),
    }
}

/// Check if a single probe value is within tolerance.
#[must_use]
pub fn is_within_tolerance(value: f64, bounds: &BaselineBounds) -> bool {
    bounds.contains(value)
}

/// Check a set of post-fault samples against baseline bounds.
/// Returns the proportion of samples within tolerance (0.0-1.0).
#[must_use]
pub fn compliance_ratio(samples: &[f64], bounds: &BaselineBounds) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let within = samples.iter().filter(|v| bounds.contains(**v)).count();
    // Both `within` and `samples.len()` are slice lengths; precision loss when
    // converting usize->f64 is acceptable for this ratio computation.
    #[allow(clippy::cast_precision_loss)]
    let ratio = within as f64 / samples.len() as f64;
    ratio
}

/// Detect the recovery point — the first index where all subsequent
/// samples are within tolerance.
#[must_use]
pub fn recovery_index(samples: &[f64], bounds: &BaselineBounds) -> Option<usize> {
    if samples.is_empty() {
        return None;
    }
    // Walk backwards to find the last breach, then recovery is the next index
    for i in (0..samples.len()).rev() {
        if !bounds.contains(samples[i]) {
            if i + 1 < samples.len() {
                return Some(i + 1);
            }
            return None; // Never recovered
        }
    }
    Some(0) // All samples within tolerance — recovered immediately
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── derive_tolerance ───────────────────────────────────────

    #[test]
    fn static_method_returns_fixed_bounds() {
        let bounds = derive_tolerance(
            &[],
            &Method::Static {
                lower: 10.0,
                upper: 90.0,
            },
        );
        assert!((bounds.lower - 10.0).abs() < f64::EPSILON);
        assert!((bounds.upper - 90.0).abs() < f64::EPSILON);
    }

    #[test]
    fn mean_stddev_derives_from_data() {
        let samples = vec![50.0; 20]; // constant = zero stddev
        let bounds = derive_tolerance(&samples, &Method::MeanStddev { sigma: 2.0 });
        assert!((bounds.lower - 50.0).abs() < f64::EPSILON);
        assert!((bounds.upper - 50.0).abs() < f64::EPSILON);
    }

    #[test]
    fn mean_stddev_widens_with_variance() {
        let samples: Vec<f64> = (1..=100).map(f64::from).collect();
        let bounds = derive_tolerance(&samples, &Method::MeanStddev { sigma: 2.0 });
        assert!(bounds.lower < 50.0);
        assert!(bounds.upper > 50.0);
        assert!(bounds.upper - bounds.lower > 50.0); // wide range
    }

    #[test]
    fn percentile_derives_upper_bound() {
        let samples: Vec<f64> = (1..=100).map(f64::from).collect();
        let bounds = derive_tolerance(
            &samples,
            &Method::Percentile {
                percentile: 95.0,
                multiplier: 1.2,
            },
        );
        assert!(bounds.lower.abs() < f64::EPSILON);
        assert!(bounds.upper > 95.0);
    }

    #[test]
    fn iqr_derives_bounds() {
        let samples: Vec<f64> = (1..=100).map(f64::from).collect();
        let bounds = derive_tolerance(&samples, &Method::Iqr);
        assert!(bounds.lower < 25.0);
        assert!(bounds.upper > 75.0);
    }

    // ── is_within_tolerance ────────────────────────────────────

    #[test]
    fn value_inside_bounds_is_within_tolerance() {
        let bounds = BaselineBounds {
            lower: 10.0,
            upper: 90.0,
        };
        assert!(is_within_tolerance(50.0, &bounds));
    }

    #[test]
    fn value_outside_bounds_is_not_within_tolerance() {
        let bounds = BaselineBounds {
            lower: 10.0,
            upper: 90.0,
        };
        assert!(!is_within_tolerance(91.0, &bounds));
    }

    // ── compliance_ratio ───────────────────────────────────────

    #[test]
    fn all_within_gives_ratio_1() {
        let bounds = BaselineBounds {
            lower: 0.0,
            upper: 100.0,
        };
        let samples = vec![10.0, 50.0, 90.0];
        assert!((compliance_ratio(&samples, &bounds) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn none_within_gives_ratio_0() {
        let bounds = BaselineBounds {
            lower: 0.0,
            upper: 10.0,
        };
        let samples = vec![20.0, 30.0, 40.0];
        assert!((compliance_ratio(&samples, &bounds) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn half_within_gives_ratio_half() {
        let bounds = BaselineBounds {
            lower: 0.0,
            upper: 50.0,
        };
        let samples = vec![10.0, 60.0, 20.0, 70.0];
        assert!((compliance_ratio(&samples, &bounds) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn empty_samples_gives_ratio_0() {
        let bounds = BaselineBounds {
            lower: 0.0,
            upper: 100.0,
        };
        assert!((compliance_ratio(&[], &bounds) - 0.0).abs() < f64::EPSILON);
    }

    // ── recovery_index ─────────────────────────────────────────

    #[test]
    fn all_within_recovers_at_index_0() {
        let bounds = BaselineBounds {
            lower: 0.0,
            upper: 100.0,
        };
        let samples = vec![10.0, 20.0, 30.0];
        assert_eq!(recovery_index(&samples, &bounds), Some(0));
    }

    #[test]
    fn breach_then_recovery() {
        let bounds = BaselineBounds {
            lower: 0.0,
            upper: 50.0,
        };
        // Breached at index 0,1,2 — recovered at index 3
        let samples = vec![100.0, 80.0, 60.0, 40.0, 30.0, 20.0];
        assert_eq!(recovery_index(&samples, &bounds), Some(3));
    }

    #[test]
    fn never_recovered_returns_none() {
        let bounds = BaselineBounds {
            lower: 0.0,
            upper: 50.0,
        };
        let samples = vec![100.0, 80.0, 60.0];
        assert_eq!(recovery_index(&samples, &bounds), None);
    }

    #[test]
    fn empty_samples_returns_none() {
        let bounds = BaselineBounds {
            lower: 0.0,
            upper: 100.0,
        };
        assert_eq!(recovery_index(&[], &bounds), None);
    }

    #[test]
    fn single_breach_at_start_then_recovery() {
        let bounds = BaselineBounds {
            lower: 0.0,
            upper: 50.0,
        };
        let samples = vec![100.0, 30.0, 20.0];
        assert_eq!(recovery_index(&samples, &bounds), Some(1));
    }
}
