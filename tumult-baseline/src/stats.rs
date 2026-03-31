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
#[inline]
#[must_use]
pub fn mean(data: &[f64]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    // Dataset lengths are at most a few thousand elements; precision loss is acceptable.
    #[allow(clippy::cast_precision_loss)]
    let len = data.len() as f64;
    data.iter().sum::<f64>() / len
}

/// Calculate the population standard deviation.
#[inline]
#[must_use]
pub fn stddev(data: &[f64]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let m = mean(data);
    // Dataset lengths are at most a few thousand elements; precision loss is acceptable.
    #[allow(clippy::cast_precision_loss)]
    let len = data.len() as f64;
    let variance = data.iter().map(|x| (x - m).powi(2)).sum::<f64>() / len;
    variance.sqrt()
}

/// Calculate a percentile value (0-100) using linear interpolation.
#[inline]
#[must_use]
pub fn percentile(data: &[f64], p: f64) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    if data.len() == 1 {
        return data[0];
    }

    let mut sorted = data.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    if p <= 0.0 {
        return sorted[0];
    }
    if p >= 100.0 {
        return sorted[sorted.len() - 1];
    }

    // Percentile rank computation: lengths are at most a few thousand elements,
    // so precision loss from usize->f64 and sign/truncation from f64->usize are acceptable.
    #[allow(clippy::cast_precision_loss)]
    let rank = (p / 100.0) * (sorted.len() - 1) as f64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lower = rank.floor() as usize;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let upper = rank.ceil() as usize;
    #[allow(clippy::cast_precision_loss)]
    let fraction = rank - lower as f64;

    sorted[lower] + fraction * (sorted[upper] - sorted[lower])
}

/// Derive tolerance bounds using mean ± N standard deviations.
#[must_use]
pub fn derive_mean_stddev_bounds(data: &[f64], sigma: f64) -> BaselineBounds {
    let m = mean(data);
    let sd = stddev(data);
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
