//! Percentiles and the bootstrap interval on a quantile difference.

use crate::rng::SplitMix64;

/// A latency quantile the verdict can be keyed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Quantile {
    P50,
    P95,
    P99,
}

impl Quantile {
    /// The quantile as a percentage in `0..=100`.
    #[must_use]
    pub fn percent(self) -> f64 {
        match self {
            Self::P50 => 50.0,
            Self::P95 => 95.0,
            Self::P99 => 99.0,
        }
    }
}

impl std::fmt::Display for Quantile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::P50 => f.write_str("p50"),
            Self::P95 => f.write_str("p95"),
            Self::P99 => f.write_str("p99"),
        }
    }
}

/// p50 / p95 / p99 of one sample set.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Percentiles {
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
}

impl Percentiles {
    #[must_use]
    pub fn get(&self, q: Quantile) -> f64 {
        match q {
            Quantile::P50 => self.p50,
            Quantile::P95 => self.p95,
            Quantile::P99 => self.p99,
        }
    }
}

/// Nearest-rank index of percentile `p` (0..=100) in a set of `n` values:
/// the `ceil(p/100 * n)`-th smallest, 1-based, clamped into range.
fn rank(p: f64, n: usize) -> usize {
    debug_assert!(n > 0);
    // n is a sample count, well below 2^53.
    #[allow(clippy::cast_precision_loss)]
    let position = (p / 100.0 * n as f64).ceil();
    // position is in 0..=n after the ceil; the cast is exact.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let index = position.max(1.0) as usize;
    index.min(n) - 1
}

/// Nearest-rank percentile of `sorted` (ascending). `None` when empty.
#[must_use]
pub fn percentile_sorted(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    Some(sorted[rank(p, sorted.len())])
}

/// Nearest-rank percentile of an unsorted scratch buffer, selecting in
/// place (O(n)) rather than sorting.
fn percentile_scratch(scratch: &mut [f64], p: f64) -> f64 {
    let i = rank(p, scratch.len());
    let (_, value, _) = scratch.select_nth_unstable_by(i, f64::total_cmp);
    *value
}

/// p50 / p95 / p99 of `values` (any order); `None` when empty.
#[must_use]
pub fn percentiles(values: &[f64]) -> Option<Percentiles> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    Some(Percentiles {
        p50: sorted[rank(50.0, sorted.len())],
        p95: sorted[rank(95.0, sorted.len())],
        p99: sorted[rank(99.0, sorted.len())],
    })
}

/// A two-sided confidence interval.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Interval {
    pub low: f64,
    pub high: f64,
}

/// Percentile bootstrap of `quantile(candidate) - quantile(baseline)`:
/// both sets are resampled with replacement `iterations` times from a
/// generator seeded with `seed`, and the interval is the central
/// `confidence` share of the resampled differences. Deterministic for the
/// same inputs. Both inputs must be non-empty and `iterations` positive.
#[must_use]
pub fn bootstrap_difference(
    baseline: &[f64],
    candidate: &[f64],
    quantile: Quantile,
    confidence: f64,
    iterations: usize,
    seed: u64,
) -> Interval {
    debug_assert!(!baseline.is_empty() && !candidate.is_empty() && iterations > 0);
    let mut rng = SplitMix64::new(seed);
    let p = quantile.percent();
    let mut base_scratch = vec![0.0; baseline.len()];
    let mut cand_scratch = vec![0.0; candidate.len()];
    let mut diffs = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        resample(baseline, &mut base_scratch, &mut rng);
        resample(candidate, &mut cand_scratch, &mut rng);
        let b = percentile_scratch(&mut base_scratch, p);
        let c = percentile_scratch(&mut cand_scratch, p);
        diffs.push(c - b);
    }
    diffs.sort_by(f64::total_cmp);
    let tail = (1.0 - confidence) / 2.0 * 100.0;
    Interval {
        low: diffs[rank(tail, diffs.len())],
        high: diffs[rank(100.0 - tail, diffs.len())],
    }
}

fn resample(from: &[f64], into: &mut [f64], rng: &mut SplitMix64) {
    for slot in into.iter_mut() {
        *slot = from[rng.below(from.len())];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_to(n: usize) -> Vec<f64> {
        // n is a small test count.
        #[allow(clippy::cast_precision_loss)]
        (1..=n).map(|i| i as f64).collect()
    }

    #[test]
    fn nearest_rank_percentiles_of_one_to_hundred() {
        let p = percentiles(&one_to(100)).expect("non-empty");
        assert!((p.p50 - 50.0).abs() < f64::EPSILON);
        assert!((p.p95 - 95.0).abs() < f64::EPSILON);
        assert!((p.p99 - 99.0).abs() < f64::EPSILON);
    }

    #[test]
    fn percentiles_of_small_and_unsorted_sets() {
        let p = percentiles(&[30.0, 10.0, 20.0]).expect("non-empty");
        assert!((p.p50 - 20.0).abs() < f64::EPSILON);
        assert!((p.p99 - 30.0).abs() < f64::EPSILON);
        let single = percentiles(&[7.0]).expect("non-empty");
        assert!((single.p50 - 7.0).abs() < f64::EPSILON);
        assert!((single.p99 - 7.0).abs() < f64::EPSILON);
        assert_eq!(percentiles(&[]), None);
        assert_eq!(percentile_sorted(&[], 50.0), None);
    }

    #[test]
    fn scratch_selection_agrees_with_sorting() {
        let values = [5.0, 1.0, 4.0, 2.0, 3.0, 9.0, 8.0];
        let mut scratch = values.to_vec();
        let selected = percentile_scratch(&mut scratch, 95.0);
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        assert!((selected - percentile_sorted(&sorted, 95.0).unwrap()).abs() < f64::EPSILON);
    }

    #[test]
    fn bootstrap_is_deterministic_for_a_seed() {
        let base = one_to(50);
        let cand: Vec<f64> = one_to(50).iter().map(|v| v + 10.0).collect();
        let a = bootstrap_difference(&base, &cand, Quantile::P95, 0.95, 500, 1);
        let b = bootstrap_difference(&base, &cand, Quantile::P95, 0.95, 500, 1);
        assert_eq!(a, b);
        let c = bootstrap_difference(&base, &cand, Quantile::P95, 0.95, 500, 2);
        let degenerate = (a.low - a.high).abs() < f64::EPSILON;
        assert!(
            a != c || degenerate,
            "a different seed should usually move the interval"
        );
    }

    #[test]
    fn bootstrap_interval_brackets_a_known_shift() {
        let base = one_to(200);
        let cand: Vec<f64> = base.iter().map(|v| v + 40.0).collect();
        let ci = bootstrap_difference(&base, &cand, Quantile::P50, 0.95, 1000, 3);
        assert!(ci.low <= 40.0 && 40.0 <= ci.high, "{ci:?}");
        assert!(ci.low >= 10.0 && ci.high <= 70.0, "{ci:?}");
    }

    #[test]
    fn bootstrap_of_identical_sets_contains_zero() {
        let base = one_to(100);
        let ci = bootstrap_difference(&base, &base, Quantile::P95, 0.95, 1000, 9);
        assert!(ci.low <= 0.0 && 0.0 <= ci.high, "{ci:?}");
    }
}
