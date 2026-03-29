//! Tumult Baseline — Statistical methods for baseline derivation.
//!
//! Provides functions to calculate statistical baselines from probe samples
//! and derive tolerance thresholds for steady-state hypothesis evaluation.

pub mod stats;

pub use stats::{
    derive_iqr_bounds, derive_mean_stddev_bounds, derive_percentile, mean, percentile, stddev,
    BaselineBounds,
};

#[cfg(test)]
mod tests {
    use super::*;

    // Known dataset: 10 samples with predictable statistics
    fn sample_data() -> Vec<f64> {
        vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0, 100.0]
    }

    // ── mean ───────────────────────────────────────────────────

    #[test]
    fn mean_of_known_dataset() {
        let data = sample_data();
        assert!((mean(&data) - 55.0).abs() < f64::EPSILON);
    }

    #[test]
    fn mean_of_single_value() {
        assert!((mean(&[42.0]) - 42.0).abs() < f64::EPSILON);
    }

    #[test]
    fn mean_of_empty_returns_zero() {
        assert!((mean(&[]) - 0.0).abs() < f64::EPSILON);
    }

    // ── stddev ─────────────────────────────────────────────────

    #[test]
    fn stddev_of_known_dataset() {
        let data = sample_data();
        // Population stddev of 10,20,...,100 = sqrt(825) ≈ 28.722
        let sd = stddev(&data);
        assert!((sd - 28.7228).abs() < 0.01);
    }

    #[test]
    fn stddev_of_constant_values_is_zero() {
        let data = vec![5.0, 5.0, 5.0, 5.0];
        assert!((stddev(&data) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn stddev_of_empty_returns_zero() {
        assert!((stddev(&[]) - 0.0).abs() < f64::EPSILON);
    }

    // ── percentile ─────────────────────────────────────────────

    #[test]
    fn p50_of_known_dataset() {
        let data = sample_data();
        let p50 = percentile(&data, 50.0);
        assert!((p50 - 55.0).abs() < 1.0);
    }

    #[test]
    fn p95_of_known_dataset() {
        let data = sample_data();
        let p95 = percentile(&data, 95.0);
        assert!((90.0..=100.0).contains(&p95));
    }

    #[test]
    fn p0_returns_minimum() {
        let data = sample_data();
        assert!((percentile(&data, 0.0) - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn p100_returns_maximum() {
        let data = sample_data();
        assert!((percentile(&data, 100.0) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn percentile_of_single_value() {
        assert!((percentile(&[42.0], 50.0) - 42.0).abs() < f64::EPSILON);
    }

    // ── derive_mean_stddev_bounds ──────────────────────────────

    #[test]
    fn mean_stddev_bounds_with_2_sigma() {
        let data = sample_data();
        let bounds = derive_mean_stddev_bounds(&data, 2.0);
        let m = 55.0;
        let sd = stddev(&data);
        assert!((bounds.lower - (m - 2.0 * sd)).abs() < 0.01);
        assert!((bounds.upper - (m + 2.0 * sd)).abs() < 0.01);
    }

    #[test]
    fn mean_stddev_bounds_with_1_sigma() {
        let data = vec![50.0, 50.0, 50.0, 50.0]; // zero stddev
        let bounds = derive_mean_stddev_bounds(&data, 1.0);
        assert!((bounds.lower - 50.0).abs() < f64::EPSILON);
        assert!((bounds.upper - 50.0).abs() < f64::EPSILON);
    }

    // ── derive_iqr_bounds ──────────────────────────────────────

    #[test]
    fn iqr_bounds_of_known_dataset() {
        let data = sample_data();
        let bounds = derive_iqr_bounds(&data);
        // Q1 ≈ 30, Q3 ≈ 80, IQR = 50
        // Lower = Q1 - 1.5*IQR = 30 - 75 = -45
        // Upper = Q3 + 1.5*IQR = 80 + 75 = 155
        assert!(bounds.lower < 0.0);
        assert!(bounds.upper > 100.0);
    }

    #[test]
    fn iqr_bounds_tight_for_constant_values() {
        let data = vec![5.0, 5.0, 5.0, 5.0, 5.0];
        let bounds = derive_iqr_bounds(&data);
        assert!((bounds.lower - 5.0).abs() < f64::EPSILON);
        assert!((bounds.upper - 5.0).abs() < f64::EPSILON);
    }

    // ── derive_percentile ──────────────────────────────────────

    #[test]
    fn percentile_threshold_with_multiplier() {
        let data = sample_data();
        let p95 = percentile(&data, 95.0);
        let threshold = derive_percentile(&data, 95.0, 1.2);
        assert!((threshold - p95 * 1.2).abs() < 0.01);
    }

    // ── BaselineBounds ─────────────────────────────────────────

    #[test]
    fn baseline_bounds_contains_value_inside() {
        let bounds = BaselineBounds {
            lower: 10.0,
            upper: 90.0,
        };
        assert!(bounds.contains(50.0));
        assert!(bounds.contains(10.0));
        assert!(bounds.contains(90.0));
    }

    #[test]
    fn baseline_bounds_rejects_value_outside() {
        let bounds = BaselineBounds {
            lower: 10.0,
            upper: 90.0,
        };
        assert!(!bounds.contains(9.9));
        assert!(!bounds.contains(90.1));
    }
}
