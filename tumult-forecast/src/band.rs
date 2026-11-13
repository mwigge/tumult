//! The narrowest band a forecast may draw.
//!
//! An empirical band comes from residuals, and a context that is constant
//! or exactly periodic has none: the band collapses to a line, a
//! normalised impact divides by zero (or by float noise, which is worse),
//! and a probe that only ever reports `200` or `true` gets no verdict at
//! all. [`floor`] is the smallest half-width worth drawing for a series:
//! a fraction of its scale, or half its observed resolution (the smallest
//! gap between two distinct values) when that is larger, and never less
//! than an absolute epsilon. [`crate::Forecast::apply_band_floor`] widens
//! a band that is narrower and marks the forecast
//! [`crate::Forecast::degenerate_band`], so every consumer can say that the
//! width is a floor and not a measurement.

use crate::stats;

/// Share of the series scale (the mean absolute value) the half-width may
/// not fall below.
pub const SCALE_FRACTION: f64 = 1e-3;
/// Absolute lower bound of the half-width, for an all-zero series.
pub const EPSILON: f64 = 1e-9;

/// The smallest half-width a band over `values` may have. Gaps and
/// infinities are ignored; an empty series gets [`EPSILON`].
#[must_use]
#[allow(clippy::cast_precision_loss)] // counts are far below 2^52
pub fn floor(values: &[f64]) -> f64 {
    let mut xs = stats::finite(values);
    if xs.is_empty() {
        return EPSILON;
    }
    let scale = xs.iter().map(|v| v.abs()).sum::<f64>() / xs.len() as f64;
    xs.sort_by(f64::total_cmp);
    let resolution = xs
        .windows(2)
        .map(|w| w[1] - w[0])
        .filter(|gap| *gap > 0.0)
        .fold(f64::INFINITY, f64::min);
    let half_resolution = if resolution.is_finite() {
        resolution / 2.0
    } else {
        0.0
    };
    (SCALE_FRACTION * scale).max(half_resolution).max(EPSILON)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn a_constant_series_floors_at_a_fraction_of_its_scale() {
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
}
