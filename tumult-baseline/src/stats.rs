//! Statistical functions for baseline derivation.

/// Upper and lower bounds for a baseline tolerance.
#[derive(Debug, Clone)]
pub struct BaselineBounds {
    pub lower: f64,
    pub upper: f64,
}

impl BaselineBounds {
    /// Check if a value falls within the bounds (inclusive).
    #[inline]
    #[must_use]
    pub fn contains(&self, value: f64) -> bool {
        (self.lower..=self.upper).contains(&value)
    }
}

/// Calculate the arithmetic mean of a dataset.
///
/// Returns `None` for an empty dataset — there is no meaningful mean of zero
/// samples, and a `0.0` sentinel silently poisons downstream CV and bounds
/// computations.
#[inline]
#[must_use]
pub fn mean(data: &[f64]) -> Option<f64> {
    if data.is_empty() {
        return None;
    }
    // Dataset lengths are at most a few thousand elements; precision loss is acceptable.
    #[allow(clippy::cast_precision_loss)]
    let len = data.len() as f64;
    Some(data.iter().sum::<f64>() / len)
}

/// Calculate the sample standard deviation (dividing by N−1, Bessel's
/// correction).
///
/// Baselines are *samples* of the system's steady-state behaviour, not the
/// full population, so the unbiased estimator is the correct one; the
/// population divisor (÷N) systematically under-estimates spread and tightens
/// tolerance bounds beyond what the data supports.
///
/// Returns `None` when fewer than two samples are available — the sample
/// variance is undefined for N < 2.
#[inline]
#[must_use]
pub fn stddev(data: &[f64]) -> Option<f64> {
    if data.len() < 2 {
        return None;
    }
    let m = mean(data)?;
    // Dataset lengths are at most a few thousand elements; precision loss is acceptable.
    #[allow(clippy::cast_precision_loss)]
    let denominator = (data.len() - 1) as f64;
    let variance = data.iter().map(|x| (x - m).powi(2)).sum::<f64>() / denominator;
    Some(variance.sqrt())
}

/// Calculate a percentile (0-100) of an ALREADY SORTED slice using linear
/// interpolation.
///
/// This is the single percentile implementation for the crate; [`percentile`]
/// is the convenience wrapper that sorts a copy first. Callers that sort once
/// for several percentile reads should use this directly.
///
/// Returns `0.0` for an empty slice.
#[must_use]
pub fn percentile_sorted(sorted: &[f64], p: f64) -> f64 {
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

/// Calculate a percentile value (0-100) using linear interpolation.
///
/// Sorts a copy of `data`; use [`percentile_sorted`] when the data is already
/// sorted or several percentiles are read from the same dataset. Returns `0.0`
/// for an empty dataset.
#[inline]
#[must_use]
pub fn percentile(data: &[f64], p: f64) -> f64 {
    if data.len() < 2 {
        return percentile_sorted(data, p);
    }
    let mut sorted = data.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    percentile_sorted(&sorted, p)
}

/// Derive tolerance bounds using mean ± N standard deviations.
///
/// Empty input yields zero-width bounds at 0.0; a single sample yields
/// zero-width bounds at that sample (its sample standard deviation is
/// undefined and treated as 0).
#[must_use]
pub fn derive_mean_stddev_bounds(data: &[f64], sigma: f64) -> BaselineBounds {
    let Some(m) = mean(data) else {
        return BaselineBounds {
            lower: 0.0,
            upper: 0.0,
        };
    };
    let sd = stddev(data).unwrap_or(0.0);
    BaselineBounds {
        lower: m - sigma * sd,
        upper: m + sigma * sd,
    }
}

/// Derive tolerance bounds using IQR (interquartile range).
///
/// Lower = Q1 - 1.5 * IQR, Upper = Q3 + 1.5 * IQR
#[must_use]
pub fn derive_iqr_bounds(data: &[f64]) -> BaselineBounds {
    let q1 = percentile(data, 25.0);
    let q3 = percentile(data, 75.0);
    let iqr = q3 - q1;
    BaselineBounds {
        lower: q1 - 1.5 * iqr,
        upper: q3 + 1.5 * iqr,
    }
}

/// Derive a percentile-based threshold with a safety multiplier.
///
/// Threshold = percentile(p) * multiplier
#[must_use]
pub fn derive_percentile(data: &[f64], p: f64, multiplier: f64) -> f64 {
    percentile(data, p) * multiplier
}
