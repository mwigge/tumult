//! The narrowest band a forecast may draw.
//!
//! An empirical band comes from residuals, and a context that is constant
//! or exactly periodic has none: the band collapses to a line, a
//! normalised impact divides by zero (or by float noise, which is worse),
//! and a probe that only ever reports `200` or `true` gets no verdict at
//! all. [`floor`] is the smallest half-width worth drawing, and it is
//! taken from the *reference* samples only — the ones the band stands for,
//! never the fault's. A floor scaled by the whole series would let thirty
//! second timeouts widen the band around a fifty millisecond baseline
//! until a post phase stuck at sixty reads as recovered.
//!
//! The floor is robust and relative to how the reference moves, not to
//! where it sits:
//!
//! * a reference with spread floors at [`SPREAD_FRACTION`] of it (the
//!   spread is half the 10-90 range, so a few outliers do not move it, and
//!   an 8e9 gauge moving by 1e6 is not floored at a share of 8e9);
//! * a reference that does not vary floors at [`LEVEL_FRACTION`] of its
//!   level (the median absolute value);
//! * either way never below half the observed resolution — the smallest
//!   gap between two distinct values inside the 10-90 range — so a boolean
//!   or a quantised gauge keeps a band its next step fits in;
//! * [`EPSILON`] only when all of that is zero (an all-zero reference).
//!
//! [`crate::Forecast::apply_band_floor`] widens a band that is narrower
//! and marks the forecast [`crate::Forecast::degenerate_band`], so every
//! consumer can say that the width is a floor and not a measurement.

use crate::stats;

/// Share of the reference's spread (half its 10-90 range) the half-width
/// may not fall below.
pub const SPREAD_FRACTION: f64 = 1e-2;
/// Share of the reference's level (its median absolute value) the
/// half-width may not fall below when the reference has no spread.
pub const LEVEL_FRACTION: f64 = 1e-3;
/// The half-width for a reference that is zero throughout.
pub const EPSILON: f64 = 1e-9;

/// The smallest half-width a band judged against `reference` may have.
/// Gaps and infinities are ignored; an empty reference gets [`EPSILON`].
#[must_use]
pub fn floor(reference: &[f64]) -> f64 {
    let xs = stats::finite(reference);
    if xs.is_empty() {
        return EPSILON;
    }
    let lower = stats::quantile(&xs, 0.1);
    let upper = stats::quantile(&xs, 0.9);
    let spread = (upper - lower) / 2.0;
    let relative = if spread > 0.0 {
        SPREAD_FRACTION * spread
    } else {
        let level: Vec<f64> = xs.iter().map(|v| v.abs()).collect();
        LEVEL_FRACTION * stats::quantile(&level, 0.5)
    };
    let central: Vec<f64> = xs
        .into_iter()
        .filter(|v| *v >= lower && *v <= upper)
        .collect();
    let half_resolution = stats::resolution(&central).map_or(0.0, |gap| gap / 2.0);
    let floor = relative.max(half_resolution);
    if floor > 0.0 && floor.is_finite() {
        floor
    } else {
        EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn a_constant_series_floors_at_a_fraction_of_its_level() {
        assert!(close(floor(&[200.0; 12]), 0.2));
        assert!(close(floor(&[-50.0, -50.0]), 0.05));
    }

    #[test]
    fn a_quantised_series_floors_at_half_its_resolution() {
        assert!(close(floor(&[1.0, 0.0, 1.0, 1.0, 0.0]), 0.5), "booleans");
        assert!(close(floor(&[300.0, 305.0, 310.0, 300.0, 305.0]), 2.5));
    }

    #[test]
    fn zeros_gaps_and_empty_series_get_the_epsilon() {
        assert!(close(floor(&[0.0, 0.0, 0.0]), EPSILON));
        assert!(close(floor(&[]), EPSILON));
        assert!(close(floor(&[f64::NAN, f64::INFINITY]), EPSILON));
        assert!(close(floor(&[f64::NAN, 4.0]), 0.004), "gaps are ignored");
    }

    /// Pseudo-noise in `[-1, 1)` with no two equal values nearby.
    fn noise(i: u32) -> f64 {
        f64::from((i * 7_919) % 1_000) / 500.0 - 1.0
    }

    /// 0.1 % of an 8e9 gauge is 8e6: eight times the noise it moves by.
    #[test]
    fn a_large_offset_series_floors_at_its_spread_not_its_level() {
        let values: Vec<f64> = (0..40).map(|i| 8e9 + noise(i) * 1e6).collect();
        let f = floor(&values);
        assert!(f > 0.0 && f < 1e5, "{f}");
    }

    #[test]
    fn a_tiny_series_is_not_floored_at_an_absolute_epsilon() {
        let values: Vec<f64> = (0..40).map(|i| 1e-7 + noise(i) * 1e-9).collect();
        let f = floor(&values);
        assert!(f > 0.0 && f < 1e-10, "{f}");
        assert!((floor(&[1e-7; 8]) - 1e-10).abs() < 1e-16, "constant");
    }

    /// A reference that holds a few fault samples keeps its own floor.
    #[test]
    fn a_few_outliers_do_not_widen_the_floor() {
        let mut values: Vec<f64> = (0..40).map(|i| 50.0 + noise(i) * 2.0).collect();
        let calm = floor(&values);
        values.extend([30_000.0, 30_000.0, 31_000.0]);
        assert!(
            (floor(&values) - calm).abs() < calm,
            "{calm} {}",
            floor(&values)
        );
        assert!(floor(&values) < 0.5);
    }
}
