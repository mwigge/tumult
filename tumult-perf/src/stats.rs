//! Percentiles and the cluster bootstrap interval on a quantile difference.

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

/// One load window: the latency it measured and the seconds of load
/// behind it. A window cut short weighs less than a whole one.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Window {
    pub value: f64,
    pub weight: f64,
}

impl Window {
    /// A window of unit weight.
    #[must_use]
    pub fn whole(value: f64) -> Self {
        Self { value, weight: 1.0 }
    }
}

/// Weighted nearest-rank percentile `p` (0..=100) of `windows`, which it
/// sorts by value: the smallest value at which the weight at or below it
/// reaches `p` percent of the total. With equal weights this is
/// [`percentile_sorted`]. `None` when empty.
#[must_use]
pub fn weighted_percentile(windows: &mut [Window], p: f64) -> Option<f64> {
    windows.sort_by(|a, b| a.value.total_cmp(&b.value));
    let total: f64 = windows.iter().map(|w| w.weight).sum();
    let wanted = p / 100.0 * total;
    let mut reached = 0.0;
    for window in windows.iter() {
        reached += window.weight;
        if reached >= wanted {
            return Some(window.value);
        }
    }
    windows.last().map(|w| w.value)
}

/// Cluster bootstrap of `quantile(candidate) - quantile(baseline)` over
/// runs: each resample draws as many whole runs per side as the side has,
/// with replacement, and pools their windows, so windows of one run stay
/// together and the interval reflects the run-to-run spread as well as
/// the spread inside a run. Deterministic for the same inputs. Both
/// sides must hold a run with a window, and `iterations` be positive.
#[must_use]
pub fn cluster_bootstrap_difference(
    baseline: &[Vec<Window>],
    candidate: &[Vec<Window>],
    quantile: Quantile,
    confidence: f64,
    iterations: usize,
    seed: u64,
) -> Interval {
    debug_assert!(!baseline.is_empty() && !candidate.is_empty() && iterations > 0);
    let mut rng = SplitMix64::new(seed);
    let p = quantile.percent();
    let mut pool = Vec::new();
    let mut diffs = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let b = resampled_percentile(baseline, &mut pool, &mut rng, p);
        let c = resampled_percentile(candidate, &mut pool, &mut rng, p);
        diffs.push(c - b);
    }
    central(&mut diffs, confidence)
}

/// The percentile of as many runs as `runs` holds, drawn whole and with
/// replacement into `pool`.
fn resampled_percentile(
    runs: &[Vec<Window>],
    pool: &mut Vec<Window>,
    rng: &mut SplitMix64,
    p: f64,
) -> f64 {
    pool.clear();
    for _ in 0..runs.len() {
        pool.extend_from_slice(&runs[rng.below(runs.len())]);
    }
    weighted_percentile(pool, p).unwrap_or(f64::NAN)
}

/// The central `confidence` share of `diffs`, which it sorts.
fn central(diffs: &mut [f64], confidence: f64) -> Interval {
    diffs.sort_by(f64::total_cmp);
    let tail = (1.0 - confidence) / 2.0 * 100.0;
    Interval {
        low: diffs[rank(tail, diffs.len())],
        high: diffs[rank(100.0 - tail, diffs.len())],
    }
}

/// A two-sided confidence interval.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Interval {
    pub low: f64,
    pub high: f64,
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
    fn a_weighted_percentile_gives_a_short_window_less_say() {
        let mut none: Vec<Window> = Vec::new();
        assert_eq!(weighted_percentile(&mut none, 50.0), None);
        assert_eq!(percentile_sorted(&[], 50.0), None);
        // Equal weights: the nearest-rank percentile.
        let mut equal: Vec<Window> = one_to(100).into_iter().rev().map(Window::whole).collect();
        for p in [0.0, 1.0, 50.0, 95.0, 99.0, 100.0] {
            let sorted = one_to(100);
            assert_eq!(
                weighted_percentile(&mut equal, p),
                percentile_sorted(&sorted, p),
                "p{p}"
            );
        }
        // Nineteen whole windows at 10 ms and one at 500 ms: the slow one
        // is the p99 when it ran as long as the others, and is not even
        // that when it was cut to a hundredth of their length.
        let mut windows: Vec<Window> = (0..19).map(|_| Window::whole(10.0)).collect();
        windows.push(Window::whole(500.0));
        assert_eq!(weighted_percentile(&mut windows, 99.0), Some(500.0));
        windows[19].weight = 0.01;
        assert_eq!(weighted_percentile(&mut windows, 99.0), Some(10.0));
        assert_eq!(weighted_percentile(&mut windows, 100.0), Some(500.0));
    }

    /// `runs` runs of `windows` AR(1) windows each around `level`: every
    /// window keeps `phi` of the one before it.
    fn autocorrelated(
        seed: u64,
        runs: usize,
        windows: usize,
        level: f64,
        phi: f64,
    ) -> Vec<Vec<Window>> {
        let mut rng = SplitMix64::new(seed);
        // 53 random bits as a uniform in [-1, 1).
        #[allow(clippy::cast_precision_loss)]
        let mut noise = move || (rng.next_u64() >> 11) as f64 / (1u64 << 52) as f64 - 1.0;
        (0..runs)
            .map(|_| {
                let mut drift = 20.0 * noise();
                (0..windows)
                    .map(|_| {
                        drift = phi * drift + 4.0 * noise();
                        Window::whole(level + drift)
                    })
                    .collect()
            })
            .collect()
    }

    fn width(ci: Interval) -> f64 {
        ci.high - ci.low
    }

    /// Every window of `runs` as a run of its own: the plain bootstrap,
    /// which takes the windows to be independent.
    fn as_independent(runs: &[Vec<Window>]) -> Vec<Vec<Window>> {
        runs.iter().flatten().map(|w| vec![*w]).collect()
    }

    #[test]
    fn the_cluster_interval_is_wider_than_the_iid_one_for_autocorrelated_windows() {
        let base = autocorrelated(11, 8, 40, 100.0, 0.95);
        let cand = autocorrelated(12, 8, 40, 100.0, 0.95);
        let iid = cluster_bootstrap_difference(
            &as_independent(&base),
            &as_independent(&cand),
            Quantile::P50,
            0.95,
            2000,
            5,
        );
        let cluster = cluster_bootstrap_difference(&base, &cand, Quantile::P50, 0.95, 2000, 5);
        assert!(
            width(cluster) > 2.0 * width(iid),
            "cluster {cluster:?} against iid {iid:?}"
        );
        // Independent windows: the two agree on the order of magnitude.
        let base = autocorrelated(11, 8, 40, 100.0, 0.0);
        let cand = autocorrelated(12, 8, 40, 100.0, 0.0);
        let independent: Vec<Vec<Window>> = base
            .iter()
            .map(|run| {
                run.iter()
                    .map(|w| Window::whole(100.0 + (w.value - 100.0) / 5.0))
                    .collect()
            })
            .collect();
        let iid = cluster_bootstrap_difference(
            &as_independent(&independent),
            &as_independent(&cand),
            Quantile::P50,
            0.95,
            2000,
            5,
        );
        let cluster =
            cluster_bootstrap_difference(&independent, &cand, Quantile::P50, 0.95, 2000, 5);
        assert!(
            width(cluster) < 2.0 * width(iid),
            "cluster {cluster:?} against iid {iid:?}"
        );
    }

    #[test]
    fn the_cluster_bootstrap_is_deterministic_and_draws_whole_runs() {
        let base = autocorrelated(1, 5, 30, 100.0, 0.9);
        let cand = autocorrelated(2, 5, 30, 110.0, 0.9);
        let a = cluster_bootstrap_difference(&base, &cand, Quantile::P95, 0.95, 500, 7);
        let b = cluster_bootstrap_difference(&base, &cand, Quantile::P95, 0.95, 500, 7);
        assert_eq!(a, b);
        // One run a side leaves nothing to draw: every resample is that
        // run, and the interval is the one difference.
        let one_base = vec![base[0].clone()];
        let one_cand = vec![cand[0].clone()];
        let ci = cluster_bootstrap_difference(&one_base, &one_cand, Quantile::P95, 0.95, 200, 7);
        assert!((ci.high - ci.low).abs() < f64::EPSILON, "{ci:?}");
        // Runs of one window each: the interval brackets a known shift.
        let singles = |values: &[f64]| -> Vec<Vec<Window>> {
            values.iter().map(|v| vec![Window::whole(*v)]).collect()
        };
        let values = one_to(200);
        let shifted: Vec<f64> = values.iter().map(|v| v + 40.0).collect();
        let ci = cluster_bootstrap_difference(
            &singles(&values),
            &singles(&shifted),
            Quantile::P50,
            0.95,
            1000,
            3,
        );
        assert!(ci.low <= 40.0 && 40.0 <= ci.high, "{ci:?}");
        assert!(ci.low >= 10.0 && ci.high <= 70.0, "{ci:?}");
    }
}
