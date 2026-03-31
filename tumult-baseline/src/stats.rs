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
    pub fn contains(&self, value: f64) -> bool {
        (self.lower..=self.upper).contains(&value)
    }
}

/// Calculate the arithmetic mean of a dataset.
#[inline]
pub fn mean(data: &[f64]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    data.iter().sum::<f64>() / data.len() as f64
}

/// Calculate the population standard deviation.
#[inline]
pub fn stddev(data: &[f64]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let m = mean(data);
    let variance = data.iter().map(|x| (x - m).powi(2)).sum::<f64>() / data.len() as f64;
    variance.sqrt()
}

/// Calculate a percentile value (0-100) using linear interpolation.
#[inline]
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

    let rank = (p / 100.0) * (sorted.len() - 1) as f64;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    let fraction = rank - lower as f64;

    sorted[lower] + fraction * (sorted[upper] - sorted[lower])
}

/// Derive tolerance bounds using mean ± N standard deviations.
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
pub fn derive_percentile(data: &[f64], p: f64, multiplier: f64) -> f64 {
    percentile(data, p) * multiplier
}
