//! Small numeric helpers shared by the providers and the derived
//! computations: finite filtering, means and interpolated empirical
//! quantiles.

/// The finite (non-`NaN`, non-infinite) values, in order.
#[must_use]
pub fn finite(values: &[f64]) -> Vec<f64> {
    values.iter().copied().filter(|v| v.is_finite()).collect()
}

/// Arithmetic mean of the finite values; `NaN` when there are none.
#[must_use]
#[allow(clippy::cast_precision_loss)] // counts are far below 2^52
pub fn mean(values: &[f64]) -> f64 {
    let xs = finite(values);
    if xs.is_empty() {
        return f64::NAN;
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// Empirical quantile of the finite values with linear interpolation
/// between order statistics (R's type 7). `NaN` when there are no finite
/// values or the level is outside `[0, 1]`.
#[must_use]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
#[allow(clippy::cast_sign_loss)] // the position is clamped to [0, n-1] first
pub fn quantile(values: &[f64], level: f64) -> f64 {
    if !(0.0..=1.0).contains(&level) {
        return f64::NAN;
    }
    let mut xs = finite(values);
    if xs.is_empty() {
        return f64::NAN;
    }
    xs.sort_by(f64::total_cmp);
    let pos = level * (xs.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    let frac = pos - pos.floor();
    xs[lo] + (xs[hi] - xs[lo]) * frac
}

/// The smallest gap between two distinct finite values: what one step of
/// a quantised series is worth. `None` when fewer than two distinct values
/// are present.
#[must_use]
pub fn resolution(values: &[f64]) -> Option<f64> {
    positive_gaps(values).into_iter().reduce(f64::min)
}

/// [`resolution`], ignoring gaps of `noise` or less: two values that
/// differ only in their last bits are one value.
#[must_use]
pub fn resolution_above(values: &[f64], noise: f64) -> Option<f64> {
    positive_gaps(values)
        .into_iter()
        .filter(|gap| *gap > noise)
        .reduce(f64::min)
}

/// How many distinct finite values there are.
#[must_use]
pub fn distinct(values: &[f64]) -> usize {
    if !values.iter().any(|v| v.is_finite()) {
        return 0;
    }
    positive_gaps(values).len() + 1
}

/// The gaps between neighbouring distinct finite values, in value order.
fn positive_gaps(values: &[f64]) -> Vec<f64> {
    let mut xs = finite(values);
    xs.sort_by(f64::total_cmp);
    xs.windows(2)
        .map(|w| w[1] - w[0])
        .filter(|gap| *gap > 0.0)
        .collect()
}

/// Every requested level's quantile, in request order.
#[must_use]
pub fn quantiles(values: &[f64], levels: &[f64]) -> Vec<f64> {
    levels.iter().map(|l| quantile(values, *l)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn finite_drops_gaps_and_infinities() {
        assert_eq!(
            finite(&[1.0, f64::NAN, 2.0, f64::INFINITY, 3.0]),
            vec![1.0, 2.0, 3.0]
        );
    }

    #[test]
    fn mean_ignores_gaps_and_is_nan_when_empty() {
        assert!(close(mean(&[1.0, f64::NAN, 3.0]), 2.0));
        assert!(mean(&[f64::NAN]).is_nan());
        assert!(mean(&[]).is_nan());
    }

    #[test]
    fn quantile_interpolates_between_order_statistics() {
        let xs = [4.0, 1.0, 3.0, 2.0];
        assert!(close(quantile(&xs, 0.0), 1.0));
        assert!(close(quantile(&xs, 1.0), 4.0));
        assert!(close(quantile(&xs, 0.5), 2.5));
        assert!(close(quantile(&xs, 0.25), 1.75));
        assert!(close(quantile(&[7.0], 0.9), 7.0));
        assert!(quantile(&xs, 1.5).is_nan());
        assert!(quantile(&[f64::NAN], 0.5).is_nan());
    }

    #[test]
    fn resolution_is_the_smallest_gap_between_distinct_values() {
        assert_eq!(resolution(&[3.0, 1.0, 1.0, 1.5, f64::NAN]), Some(0.5));
        assert_eq!(resolution(&[2.0, 2.0]), None);
        assert_eq!(resolution(&[]), None);
        assert_eq!(distinct(&[3.0, 1.0, 1.0, 1.5, f64::NAN]), 3);
        assert_eq!(distinct(&[2.0, 2.0]), 1);
        assert_eq!(distinct(&[f64::NAN]), 0);
        assert_eq!(resolution_above(&[1.0, 1.25, 3.0], 0.25), Some(1.75));
        assert_eq!(resolution_above(&[1.0, 1.25], 0.25), None);
    }

    #[test]
    fn quantiles_follow_request_order() {
        let q = quantiles(&[1.0, 2.0, 3.0], &[0.9, 0.5]);
        assert!(close(q[0], 2.8));
        assert!(close(q[1], 2.0));
    }
}
