//! Performance-regression verdicts.
//!
//! Given a baseline and a candidate set of latencies (typically the
//! `during`-phase load samples of the last two `service.version`s of a
//! target), [`compare`] reports p50/p95/p99 of each set, the difference of
//! the chosen quantile with a seeded bootstrap confidence interval, and a
//! verdict:
//!
//! * `regressed` — the whole interval lies above the noise margin;
//! * `improved` — the whole interval lies below minus the margin;
//! * `unchanged` — the interval is within the margin, or contains zero
//!   without reaching past the margin on both sides;
//! * `inconclusive` — too few samples, or an interval so wide it reaches
//!   past the margin in both directions.
//!
//! Everything here is pure and deterministic: the same inputs and options
//! give the same interval and verdict.

mod rng;
mod stats;

pub use rng::SplitMix64;
pub use stats::{
    bootstrap_difference, percentile_sorted, percentiles, Interval, Percentiles, Quantile,
};

/// The outcome of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Improved,
    Unchanged,
    Regressed,
    Inconclusive,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Improved => f.write_str("improved"),
            Self::Unchanged => f.write_str("unchanged"),
            Self::Regressed => f.write_str("regressed"),
            Self::Inconclusive => f.write_str("inconclusive"),
        }
    }
}

/// Knobs of a comparison; [`CompareOptions::default`] is the documented
/// default (p95, 95 % confidence, 2000 resamples, 20 samples minimum, a
/// 5 % noise margin).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CompareOptions {
    /// Quantile the verdict is keyed on.
    pub quantile: Quantile,
    /// Confidence level of the bootstrap interval, in `(0, 1)`.
    pub confidence: f64,
    /// Bootstrap resamples.
    pub iterations: usize,
    /// Seed of the bootstrap generator.
    pub seed: u64,
    /// Fewer samples on either side is `inconclusive`.
    pub min_samples: usize,
    /// Noise band as a percentage of the baseline quantile; a difference
    /// inside it is `unchanged` however significant.
    pub noise_margin_pct: f64,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self {
            quantile: Quantile::P95,
            confidence: 0.95,
            iterations: 2000,
            seed: 0x5EED_0000_0000_0001,
            min_samples: 20,
            noise_margin_pct: 5.0,
        }
    }
}

/// Summary of one side of the comparison.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SampleSummary {
    /// Finite samples used.
    pub n: usize,
    /// Non-finite inputs that were dropped.
    pub dropped: usize,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
}

impl SampleSummary {
    fn of(values: &[f64], dropped: usize) -> Self {
        let p = stats::percentiles(values);
        Self {
            n: values.len(),
            dropped,
            p50: p.map(|p| p.p50),
            p95: p.map(|p| p.p95),
            p99: p.map(|p| p.p99),
        }
    }

    fn quantile(&self, q: Quantile) -> Option<f64> {
        match q {
            Quantile::P50 => self.p50,
            Quantile::P95 => self.p95,
            Quantile::P99 => self.p99,
        }
    }
}

/// The full answer of [`compare`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Comparison {
    pub quantile: Quantile,
    pub baseline: SampleSummary,
    pub candidate: SampleSummary,
    /// `candidate − baseline` at `quantile`, in the samples' unit.
    pub delta: Option<f64>,
    /// `delta` relative to the baseline quantile, in percent.
    pub delta_pct: Option<f64>,
    /// Bootstrap interval on `delta`; absent when inconclusive for lack of
    /// samples.
    pub interval: Option<Interval>,
    pub confidence: f64,
    /// The noise band in the samples' unit.
    pub margin: f64,
    pub iterations: usize,
    pub seed: u64,
    pub verdict: Verdict,
    /// Why the verdict is what it is.
    pub reasons: Vec<String>,
}

/// The finite values of `values` and how many were dropped.
fn finite(values: &[f64]) -> (Vec<f64>, usize) {
    let kept: Vec<f64> = values.iter().copied().filter(|v| v.is_finite()).collect();
    let dropped = values.len() - kept.len();
    (kept, dropped)
}

/// Compare `candidate` against `baseline`.
#[must_use]
pub fn compare(baseline: &[f64], candidate: &[f64], opts: &CompareOptions) -> Comparison {
    let (base, base_dropped) = finite(baseline);
    let (cand, cand_dropped) = finite(candidate);
    let baseline = SampleSummary::of(&base, base_dropped);
    let candidate = SampleSummary::of(&cand, cand_dropped);
    let q = opts.quantile;
    let mut reasons = Vec::new();
    for (side, s) in [("baseline", &baseline), ("candidate", &candidate)] {
        if s.dropped > 0 {
            reasons.push(format!(
                "{side}: {} non-finite sample(s) dropped",
                s.dropped
            ));
        }
    }
    let margin = baseline
        .quantile(q)
        .map_or(0.0, |b| b.abs() * opts.noise_margin_pct / 100.0);
    let delta = match (baseline.quantile(q), candidate.quantile(q)) {
        (Some(b), Some(c)) => Some(c - b),
        _ => None,
    };
    let delta_pct = match (delta, baseline.quantile(q)) {
        (Some(d), Some(b)) if b != 0.0 => Some(d / b * 100.0),
        _ => None,
    };
    let done = |verdict, interval, reasons| Comparison {
        quantile: q,
        baseline,
        candidate,
        delta,
        delta_pct,
        interval,
        confidence: opts.confidence,
        margin,
        iterations: opts.iterations,
        seed: opts.seed,
        verdict,
        reasons,
    };
    let too_few = [("baseline", baseline.n), ("candidate", candidate.n)]
        .into_iter()
        .filter(|(_, n)| *n < opts.min_samples)
        .map(|(side, n)| {
            format!(
                "{side} has {n} sample(s); at least {} needed",
                opts.min_samples
            )
        })
        .collect::<Vec<_>>();
    if !too_few.is_empty() || opts.iterations == 0 {
        reasons.extend(too_few);
        if opts.iterations == 0 {
            reasons.push("bootstrap iterations is 0".into());
        }
        return done(Verdict::Inconclusive, None, reasons);
    }
    let ci =
        stats::bootstrap_difference(&base, &cand, q, opts.confidence, opts.iterations, opts.seed);
    let (verdict, reason) = classify(ci, margin, q);
    reasons.push(reason);
    done(verdict, Some(ci), reasons)
}

/// The verdict rule on an interval and a noise margin.
fn classify(ci: Interval, margin: f64, q: Quantile) -> (Verdict, String) {
    let band = format!(
        "[{:.2}, {:.2}] against a noise margin of ±{margin:.2}",
        ci.low, ci.high
    );
    if ci.low > margin {
        return (
            Verdict::Regressed,
            format!("{q} difference interval {band} lies entirely above the margin"),
        );
    }
    if ci.high < -margin {
        return (
            Verdict::Improved,
            format!("{q} difference interval {band} lies entirely below the margin"),
        );
    }
    if ci.low < -margin && ci.high > margin {
        return (
            Verdict::Inconclusive,
            format!("{q} difference interval {band} reaches past the margin in both directions"),
        );
    }
    (
        Verdict::Unchanged,
        format!("{q} difference interval {band} is within the margin or includes zero"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn around(center: f64, n: usize, spread: f64) -> Vec<f64> {
        // Deterministic jitter: i goes to n, a small test count.
        #[allow(clippy::cast_precision_loss)]
        (0..n)
            .map(|i| center + spread * (((i * 7919) % 13) as f64 - 6.0) / 6.0)
            .collect()
    }

    #[test]
    fn identical_sets_are_unchanged() {
        let base = around(100.0, 60, 5.0);
        let c = compare(&base, &base, &CompareOptions::default());
        assert_eq!(c.verdict, Verdict::Unchanged, "{c:?}");
        let ci = c.interval.expect("interval");
        assert!(ci.low <= 0.0 && 0.0 <= ci.high);
        assert_eq!(c.delta, Some(0.0));
        assert_eq!(c.baseline.n, 60);
        assert!(c.reasons[0].contains("includes zero") || c.reasons[0].contains("within"));
    }

    #[test]
    fn a_clear_slowdown_is_regressed_with_a_margin() {
        let base = around(100.0, 60, 5.0);
        let cand = around(150.0, 60, 5.0);
        let c = compare(&base, &cand, &CompareOptions::default());
        assert_eq!(c.verdict, Verdict::Regressed, "{c:?}");
        let ci = c.interval.expect("interval");
        assert!(ci.low > c.margin);
        assert!((c.margin - 0.05 * c.baseline.p95.unwrap()).abs() < 1e-9);
        assert!(c.delta_pct.unwrap() > 40.0);
    }

    #[test]
    fn a_clear_speedup_is_improved() {
        let base = around(100.0, 60, 5.0);
        let cand = around(60.0, 60, 5.0);
        let c = compare(&base, &cand, &CompareOptions::default());
        assert_eq!(c.verdict, Verdict::Improved, "{c:?}");
        assert!(c.interval.unwrap().high < -c.margin);
    }

    #[test]
    fn a_shift_inside_the_noise_margin_is_unchanged() {
        let base = around(100.0, 200, 1.0);
        let cand = around(102.0, 200, 1.0);
        let opts = CompareOptions {
            noise_margin_pct: 10.0,
            ..CompareOptions::default()
        };
        let c = compare(&base, &cand, &opts);
        assert_eq!(c.verdict, Verdict::Unchanged, "{c:?}");
    }

    #[test]
    fn too_few_samples_is_inconclusive_with_a_reason() {
        let base = around(100.0, 5, 5.0);
        let cand = around(150.0, 60, 5.0);
        let c = compare(&base, &cand, &CompareOptions::default());
        assert_eq!(c.verdict, Verdict::Inconclusive);
        assert!(c.interval.is_none());
        assert!(
            c.reasons.iter().any(|r| r.contains("baseline has 5")),
            "{:?}",
            c.reasons
        );
        // Percentiles are still reported for what there is.
        assert!(c.baseline.p95.is_some());
        assert!(c.delta.is_some());
    }

    #[test]
    fn empty_sides_report_no_percentiles() {
        let c = compare(&[], &[], &CompareOptions::default());
        assert_eq!(c.verdict, Verdict::Inconclusive);
        assert_eq!(c.baseline.p50, None);
        assert_eq!(c.delta, None);
        assert_eq!(c.delta_pct, None);
    }

    #[test]
    fn an_interval_spanning_both_directions_is_inconclusive() {
        let base = vec![100.0; 40];
        let mut cand = vec![50.0; 20];
        cand.extend(vec![150.0; 20]);
        let opts = CompareOptions {
            quantile: Quantile::P50,
            ..CompareOptions::default()
        };
        let c = compare(&base, &cand, &opts);
        assert_eq!(c.verdict, Verdict::Inconclusive, "{c:?}");
        assert!(c.reasons.last().unwrap().contains("both directions"));
    }

    #[test]
    fn non_finite_samples_are_dropped_and_reported() {
        let mut base = around(100.0, 60, 5.0);
        base.push(f64::NAN);
        base.push(f64::INFINITY);
        let c = compare(&base, &base, &CompareOptions::default());
        assert_eq!(c.baseline.dropped, 2);
        assert_eq!(c.baseline.n, 60);
        assert!(c.reasons.iter().any(|r| r.contains("non-finite")));
    }

    #[test]
    fn comparison_is_deterministic_and_serialisable() {
        let base = around(100.0, 60, 5.0);
        let cand = around(120.0, 60, 5.0);
        let a = compare(&base, &cand, &CompareOptions::default());
        let b = compare(&base, &cand, &CompareOptions::default());
        assert_eq!(a, b);
        let json = serde_json::to_value(&a).expect("serialise");
        assert_eq!(json["verdict"], "regressed");
        assert_eq!(json["quantile"], "p95");
        let back: Comparison = serde_json::from_value(json).expect("round trip");
        assert_eq!(back, a);
    }

    #[test]
    fn zero_iterations_is_inconclusive() {
        let base = around(100.0, 60, 5.0);
        let opts = CompareOptions {
            iterations: 0,
            ..CompareOptions::default()
        };
        let c = compare(&base, &base, &opts);
        assert_eq!(c.verdict, Verdict::Inconclusive);
        assert!(c.reasons.iter().any(|r| r.contains("iterations")));
    }

    #[test]
    fn verdict_and_quantile_display() {
        assert_eq!(Verdict::Regressed.to_string(), "regressed");
        assert_eq!(Quantile::P99.to_string(), "p99");
        assert!((Quantile::P99.percent() - 99.0).abs() < f64::EPSILON);
    }
}
