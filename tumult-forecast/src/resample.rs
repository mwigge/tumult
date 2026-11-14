//! The series contract: irregular `(ts_ns, value)` points become a
//! regular-step series with `NaN` gaps, at most [`MAX_POINTS`] long.
//!
//! Every provider — the built-in one and the Timestral sidecar alike —
//! expects exactly this shape. The step is chosen from the window: one
//! second for a run, one minute for a day, one hour for a quarter (the
//! first tier that keeps the window under the cap).

/// Longest context any provider accepts (Timestral's limit, and plenty for
/// the built-in provider).
pub const MAX_POINTS: usize = 32_768;

const SECOND_NS: i64 = 1_000_000_000;
const MINUTE_NS: i64 = 60 * SECOND_NS;
const HOUR_NS: i64 = 3_600 * SECOND_NS;

/// The step tiers, finest first.
pub const STEP_TIERS_NS: [i64; 3] = [SECOND_NS, MINUTE_NS, HOUR_NS];

/// A regular-step series. `values[i]` is the bucket mean over
/// `[start_ns + i * step_ns, start_ns + (i + 1) * step_ns)`, `NaN` when the
/// bucket is empty.
#[derive(Debug, Clone, PartialEq)]
pub struct RegularSeries {
    pub start_ns: i64,
    pub step_ns: i64,
    pub values: Vec<f64>,
}

impl RegularSeries {
    /// Number of steps.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether the series has no steps.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Timestamp at the start of step `i`.
    #[must_use]
    pub fn ts_ns(&self, i: usize) -> i64 {
        self.start_ns + step_offset(i, self.step_ns)
    }

    /// Timestamp one step past the end.
    #[must_use]
    pub fn end_ns(&self) -> i64 {
        self.ts_ns(self.len())
    }

    /// The step containing `ts_ns`, when it lies inside the series.
    #[must_use]
    pub fn index_of(&self, ts_ns: i64) -> Option<usize> {
        if self.step_ns <= 0 || ts_ns < self.start_ns || ts_ns >= self.end_ns() {
            return None;
        }
        usize::try_from((ts_ns - self.start_ns) / self.step_ns).ok()
    }

    /// Every step's start timestamp, in epoch milliseconds (chart axes).
    #[must_use]
    pub fn ts_ms(&self) -> Vec<i64> {
        (0..self.len()).map(|i| self.ts_ns(i) / 1_000_000).collect()
    }
}

fn step_offset(i: usize, step_ns: i64) -> i64 {
    i64::try_from(i).map_or(i64::MAX, |i| i.saturating_mul(step_ns))
}

/// The finest tier that keeps `window_ns` under [`MAX_POINTS`] steps; the
/// coarsest tier when none does (the resampler then keeps the latest
/// [`MAX_POINTS`] buckets).
#[must_use]
pub fn step_for_window(window_ns: i64) -> i64 {
    let window = window_ns.max(0);
    STEP_TIERS_NS
        .iter()
        .copied()
        .find(|step| ceil_div(window, *step) <= max_points_i64())
        .unwrap_or(HOUR_NS)
}

/// `ceil(a / b)` for non-negative `a` and positive `b`.
fn ceil_div(a: i64, b: i64) -> i64 {
    (a + b - 1) / b
}

fn max_points_i64() -> i64 {
    i64::try_from(MAX_POINTS).unwrap_or(i64::MAX)
}

/// Resample points into `[from_ns, to_ns)` at `step_ns`. Points outside the
/// window are dropped; non-finite values are gaps; a bucket's value is the
/// mean of its points. When the window holds more than [`MAX_POINTS`]
/// buckets, the latest ones are kept.
#[must_use]
pub fn resample(points: &[(i64, f64)], from_ns: i64, to_ns: i64, step_ns: i64) -> RegularSeries {
    if step_ns <= 0 || to_ns <= from_ns {
        return RegularSeries {
            start_ns: from_ns,
            step_ns: step_ns.max(1),
            values: Vec::new(),
        };
    }
    let total = usize::try_from(ceil_div(to_ns - from_ns, step_ns)).unwrap_or(usize::MAX);
    let keep = total.min(MAX_POINTS);
    let start_ns = from_ns + step_offset(total - keep, step_ns);
    let mut sums = vec![0.0_f64; keep];
    let mut counts = vec![0_u32; keep];
    for &(ts, value) in points {
        if !value.is_finite() || ts < start_ns || ts >= to_ns {
            continue;
        }
        let Ok(idx) = usize::try_from((ts - start_ns) / step_ns) else {
            continue;
        };
        if idx >= keep {
            continue;
        }
        sums[idx] += value;
        counts[idx] += 1;
    }
    let values = sums
        .iter()
        .zip(&counts)
        .map(|(sum, count)| {
            if *count == 0 {
                f64::NAN
            } else {
                sum / f64::from(*count)
            }
        })
        .collect();
    RegularSeries {
        start_ns,
        step_ns,
        values,
    }
}

/// [`resample`] at the step [`step_for_window`] picks for the window.
#[must_use]
pub fn resample_auto(points: &[(i64, f64)], from_ns: i64, to_ns: i64) -> RegularSeries {
    resample(points, from_ns, to_ns, step_for_window(to_ns - from_ns))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_tiers_follow_the_window() {
        assert_eq!(step_for_window(20 * MINUTE_NS), SECOND_NS, "a run");
        assert_eq!(step_for_window(24 * HOUR_NS), MINUTE_NS, "a day");
        assert_eq!(step_for_window(90 * 24 * HOUR_NS), HOUR_NS, "a quarter");
        assert_eq!(
            step_for_window(10 * 365 * 24 * HOUR_NS),
            HOUR_NS,
            "beyond the cap"
        );
        assert_eq!(step_for_window(0), SECOND_NS);
        assert_eq!(step_for_window(-5), SECOND_NS);
    }

    #[test]
    fn buckets_average_and_gaps_are_nan() {
        let points = [
            (1_000 * SECOND_NS, 2.0),
            (1_000 * SECOND_NS + 10, 4.0),
            (1_002 * SECOND_NS, 7.0),
            (1_003 * SECOND_NS, f64::NAN),
            (999 * SECOND_NS, 100.0),   // before the window
            (1_004 * SECOND_NS, 100.0), // at the end, exclusive
        ];
        let s = resample(&points, 1_000 * SECOND_NS, 1_004 * SECOND_NS, SECOND_NS);
        assert_eq!(s.start_ns, 1_000 * SECOND_NS);
        assert_eq!(s.step_ns, SECOND_NS);
        assert_eq!(s.len(), 4);
        assert!((s.values[0] - 3.0).abs() < 1e-12);
        assert!(s.values[1].is_nan());
        assert!((s.values[2] - 7.0).abs() < 1e-12);
        assert!(s.values[3].is_nan(), "non-finite values are gaps");
        assert_eq!(s.index_of(1_002 * SECOND_NS + 5), Some(2));
        assert_eq!(s.index_of(1_004 * SECOND_NS), None);
        assert_eq!(s.index_of(999 * SECOND_NS), None);
        assert_eq!(s.ts_ms(), vec![1_000_000, 1_001_000, 1_002_000, 1_003_000]);
        assert_eq!(s.end_ns(), 1_004 * SECOND_NS);
    }

    #[test]
    fn partial_last_bucket_is_kept() {
        let s = resample(&[], 0, SECOND_NS + 1, SECOND_NS);
        assert_eq!(s.len(), 2);
        assert!(s.values.iter().all(|v| v.is_nan()));
    }

    #[test]
    fn empty_or_inverted_windows_yield_no_steps() {
        assert!(resample(&[(0, 1.0)], 10, 10, SECOND_NS).is_empty());
        assert!(resample(&[(0, 1.0)], 10, 0, SECOND_NS).is_empty());
        assert!(resample(&[(0, 1.0)], 0, 10, 0).is_empty());
    }

    #[test]
    fn cap_keeps_the_latest_points() {
        let to = i64::try_from(MAX_POINTS + 10).unwrap() * SECOND_NS;
        let points = [(0, 1.0), (to - SECOND_NS, 9.0)];
        let s = resample(&points, 0, to, SECOND_NS);
        assert_eq!(s.len(), MAX_POINTS);
        assert_eq!(s.start_ns, 10 * SECOND_NS);
        assert!((s.values[MAX_POINTS - 1] - 9.0).abs() < 1e-12);
        assert!(s.values[0].is_nan(), "the dropped early point is gone");
    }

    #[test]
    fn auto_picks_the_window_step() {
        let s = resample_auto(&[(0, 1.0)], 0, 2 * 24 * HOUR_NS);
        assert_eq!(s.step_ns, MINUTE_NS);
        assert_eq!(s.len(), 2 * 24 * 60);
    }
}
