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
//! * strays are set aside first. The reference's *core* is what lies
//!   within [`CORE_WIDTHS`] interquartile ranges of its quartiles, or,
//!   when the quartiles coincide, within one level (the median absolute
//!   value) of the median. Thirty second timeouts from an earlier incident
//!   are not the spread of a fifty millisecond service, however many of
//!   them the baseline holds short of a quarter;
//! * a difference of [`NOISE_FRACTION`] of the level or less is float
//!   noise, neither a spread nor a step: a mean of the same number that
//!   differs in its last bit is still a constant;
//! * a core with spread floors at [`SPREAD_FRACTION`] of it (the spread is
//!   half the 10-90 range, so an 8e9 gauge moving by 1e6 is not floored at
//!   a share of 8e9);
//! * a core that does not vary floors at [`LEVEL_FRACTION`] of the level;
//! * either way never below half the observed step — the smallest gap
//!   between two distinct values anywhere in the core — so a boolean, a
//!   quantised gauge or a `200` seen once as `201` keeps a band its next
//!   step fits in;
//! * [`EPSILON`] only when all of that is zero (an all-zero reference).
//!
//! A reference that never moved shows no step of its own. [`floor_within`]
//! then takes the smallest step of the series the reference is part of,
//! when the level makes it plausible (a step of 1 for a boolean, not the
//! 300 between a `200` and a `500`). Half of the smallest gap in the
//! series can never reach another value of the series, so the fault's own
//! scale does not hide the fault.
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
/// Share of the level below which a difference is float noise.
pub const NOISE_FRACTION: f64 = 1e-9;
/// Interquartile ranges beyond the quartiles a value may lie and still
/// belong to the reference's core.
pub const CORE_WIDTHS: f64 = 3.0;

/// The smallest half-width a band judged against `reference` may have.
/// Gaps and infinities are ignored; an empty reference gets [`EPSILON`].
#[must_use]
pub fn floor(reference: &[f64]) -> f64 {
    Core::of(reference).map_or(EPSILON, |core| core.floor(core.step()))
}

/// [`floor`], for a reference that is part of `series`: a reference with
/// no step of its own takes a plausible one from the series.
#[must_use]
pub fn floor_within(reference: &[f64], series: &[f64]) -> f64 {
    let Some(core) = Core::of(reference) else {
        return EPSILON;
    };
    let step = core.step().or_else(|| core.borrowed_step(series));
    core.floor(step)
}

/// The reference without its strays, and the level it sits at.
struct Core {
    values: Vec<f64>,
    level: f64,
}

impl Core {
    fn of(reference: &[f64]) -> Option<Self> {
        let xs = stats::finite(reference);
        if xs.is_empty() {
            return None;
        }
        let magnitudes: Vec<f64> = xs.iter().map(|v| v.abs()).collect();
        let level = stats::quantile(&magnitudes, 0.5);
        let (lower, upper) = Self::bounds(&xs, level);
        let values = xs
            .into_iter()
            .filter(|v| *v >= lower && *v <= upper)
            .collect();
        Some(Self { values, level })
    }

    /// Where the core ends: an interquartile fence, or one level around
    /// the median when the quartiles coincide. A zero level has no scale
    /// to judge a stray by and keeps everything.
    fn bounds(xs: &[f64], level: f64) -> (f64, f64) {
        let lower = stats::quantile(xs, 0.25);
        let upper = stats::quantile(xs, 0.75);
        let width = upper - lower;
        if width > NOISE_FRACTION * level {
            return (lower - CORE_WIDTHS * width, upper + CORE_WIDTHS * width);
        }
        if level > 0.0 {
            let median = stats::quantile(xs, 0.5);
            return (median - level, median + level);
        }
        (f64::NEG_INFINITY, f64::INFINITY)
    }

    fn noise(&self) -> f64 {
        NOISE_FRACTION * self.level
    }

    /// The smallest gap between two values of the core that is not noise.
    fn step(&self) -> Option<f64> {
        stats::resolution_above(&self.values, self.noise())
    }

    /// The smallest step of `series`, when the level makes it plausible.
    fn borrowed_step(&self, series: &[f64]) -> Option<f64> {
        stats::resolution_above(series, self.noise())
            .filter(|gap| self.level <= 0.0 || *gap <= self.level)
    }

    fn relative(&self) -> f64 {
        let lower = stats::quantile(&self.values, 0.1);
        let upper = stats::quantile(&self.values, 0.9);
        let spread = (upper - lower) / 2.0;
        if spread > self.noise() {
            return SPREAD_FRACTION * spread;
        }
        LEVEL_FRACTION * self.level
    }

    fn floor(&self, step: Option<f64>) -> f64 {
        let floor = self.relative().max(step.map_or(0.0, |gap| gap / 2.0));
        if floor > 0.0 && floor.is_finite() {
            return floor;
        }
        EPSILON
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    /// A constant whose samples are means of the same number: the last
    /// bit differs, and that is not a spread.
    #[test]
    fn float_noise_around_a_constant_is_not_a_spread() {
        let noisy = 0.1 + f64::EPSILON / 8.0;
        assert!(noisy > 0.1, "the two values differ in the last bit");
        let values = [0.1, noisy, 0.1, noisy, noisy, 0.1, 0.1, noisy, 0.1, noisy];
        assert!(close(floor(&values), 1e-4), "{}", floor(&values));
        assert!(close(floor(&[0.1, noisy]), 1e-4), "two samples");
    }

    /// A value seen once next to the level is one step of the metric.
    #[test]
    fn a_rare_neighbour_of_the_level_sets_the_resolution() {
        assert!(close(floor(&[200.0, 200.0, 200.0, 200.0, 201.0]), 0.5));
        let mut values = vec![53.0; 40];
        values.extend([54.0; 3]);
        assert!(close(floor(&values), 0.5), "{}", floor(&values));
        assert!((54.0 - 53.0) / floor(&values) <= 2.0, "54 is the next step");
    }

    #[test]
    fn two_or_three_samples_floor_at_half_their_step() {
        assert!(close(floor(&[0.0, 1.0, 1.0]), 0.5));
        assert!(close(floor(&[3.0, 7.0]), 2.0));
        assert!(close(floor(&[7.0, 3.0]), 2.0));
    }

    /// Thirty second timeouts in six of forty baseline samples: an earlier
    /// incident, not the spread of a fifty millisecond service.
    #[test]
    fn an_incident_in_the_reference_does_not_widen_the_floor() {
        let calm: Vec<f64> = (0..34).map(|i| 50.0 + noise(i) * 2.0).collect();
        let mut values = calm.clone();
        for at in [3, 9, 15, 21, 27, 33] {
            values.insert(at, 30_000.0);
        }
        assert!(values.len() == 40 && floor(&calm) < 0.1);
        assert!(
            (floor(&values) - floor(&calm)).abs() < floor(&calm),
            "{} against {}",
            floor(&values),
            floor(&calm)
        );
    }

    #[test]
    fn one_spike_in_eight_samples_does_not_move_the_floor() {
        let flat = [50.0, 50.0, 50.0, 30_000.0, 50.0, 50.0, 50.0, 50.0];
        assert!(close(floor(&flat), 0.05), "{}", floor(&flat));
        let near = [50.0, 50.0, 50.0, 190.0, 50.0, 50.0, 50.0, 50.0];
        assert!(close(floor(&near), 0.05), "{}", floor(&near));
        let calm: Vec<f64> = (0..7).map(|i| 50.0 + noise(i) * 2.0).collect();
        let mut spiked = calm.clone();
        spiked.insert(4, 30_000.0);
        assert!((floor(&spiked) - floor(&calm)).abs() < floor(&calm));
    }

    /// A reference that never moved has no step of its own; the series it
    /// is part of shows the metric's grid, when the step is one the level
    /// makes plausible.
    #[test]
    fn a_constant_reference_takes_a_plausible_step_from_its_series() {
        let boolean = [1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0];
        assert!(close(floor_within(&boolean[3..], &boolean), 0.5));
        assert!(close(floor_within(&boolean[..3], &boolean), 0.5));
        let status = [200.0, 200.0, 200.0, 500.0, 500.0, 200.0];
        assert!(
            close(floor_within(&status[..3], &status), 0.2),
            "300 is no step of 200"
        );
        let latency = [50.0, 50.0, 50.0, 30_000.0, 30_000.5, 50.0];
        assert!(close(floor_within(&latency[..3], &latency), 0.25));
        let moving = [10.0, 10.5, 11.0, 10.5, 90.0, 90.1];
        assert!(
            close(floor_within(&moving[..4], &moving), floor(&moving[..4])),
            "a reference with a step keeps its own"
        );
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
