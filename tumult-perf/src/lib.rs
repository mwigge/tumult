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
    bootstrap_difference, cluster_bootstrap_difference, percentile_sorted, percentiles,
    weighted_percentile, Interval, Percentiles, Quantile, Window,
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

/// The fewest runs a side needs for a verdict other than `inconclusive`,
/// unless the caller says otherwise.
pub const DEFAULT_MIN_RUNS: usize = 3;

/// The runs a side needs for its interval to be called settled. The
/// interval resamples whole runs: three runs can be drawn in ten distinct
/// ways, four in 35, five in 126. A verdict from fewer is still a
/// verdict (the default minimum stays [`DEFAULT_MIN_RUNS`], so that a
/// build can be judged after three runs), and says so in
/// [`Comparison::low_run_count`].
pub const SETTLED_RUNS: usize = 5;

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
    /// Fewer runs with samples on either side is `inconclusive`: the
    /// interval is drawn from whole runs, and one or two give it nothing
    /// to draw from.
    pub min_runs: usize,
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
            min_runs: DEFAULT_MIN_RUNS,
            noise_margin_pct: 5.0,
        }
    }
}

/// Summary of one side of the comparison.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SampleSummary {
    /// Finite samples used.
    pub n: usize,
    /// Runs that hold at least one of them.
    #[serde(default)]
    pub runs: usize,
    /// Non-finite inputs that were dropped.
    pub dropped: usize,
    pub p50: Option<f64>,
    pub p95: Option<f64>,
    pub p99: Option<f64>,
}

impl SampleSummary {
    fn of(runs: &[Vec<Window>], dropped: usize) -> Self {
        let mut all: Vec<Window> = runs.iter().flatten().copied().collect();
        Self {
            n: all.len(),
            runs: runs.len(),
            dropped,
            p50: stats::weighted_percentile(&mut all, 50.0),
            p95: stats::weighted_percentile(&mut all, 95.0),
            p99: stats::weighted_percentile(&mut all, 99.0),
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
    /// The interval was drawn with fewer than [`SETTLED_RUNS`] runs on a
    /// side: few distinct resamples, so read its bounds as coarse.
    /// `false` when no interval was drawn.
    #[serde(default)]
    pub low_run_count: bool,
    /// Why the verdict is what it is.
    pub reasons: Vec<String>,
}

/// Whether a window says anything: a finite value behind a positive,
/// finite duration.
fn usable(window: &Window) -> bool {
    window.value.is_finite() && window.weight.is_finite() && window.weight > 0.0
}

/// The usable windows of `runs`, run by run without the runs left empty,
/// and how many windows were dropped.
fn usable_runs(runs: &[Vec<Window>]) -> (Vec<Vec<Window>>, usize) {
    let mut dropped = 0;
    let mut kept = Vec::with_capacity(runs.len());
    for run in runs {
        let windows: Vec<Window> = run.iter().copied().filter(usable).collect();
        dropped += run.len() - windows.len();
        if !windows.is_empty() {
            kept.push(windows);
        }
    }
    (kept, dropped)
}

/// Compare independent samples: every value is a run of its own, so the
/// interval is the plain bootstrap. For load windows use
/// [`compare_runs`]; windows of one run are not independent.
#[must_use]
pub fn compare(baseline: &[f64], candidate: &[f64], opts: &CompareOptions) -> Comparison {
    let singles = |values: &[f64]| -> Vec<Vec<Window>> {
        values.iter().map(|v| vec![Window::whole(*v)]).collect()
    };
    compare_runs(&singles(baseline), &singles(candidate), opts)
}

/// Compare the runs of `candidate` against those of `baseline`, each run
/// its windows in order. Percentiles weigh a window by its duration, and
/// the interval is a cluster bootstrap over runs
/// ([`cluster_bootstrap_difference`]): a side needs `min_runs` runs and
/// `min_samples` windows for a verdict other than `inconclusive`.
#[must_use]
pub fn compare_runs(
    baseline: &[Vec<Window>],
    candidate: &[Vec<Window>],
    opts: &CompareOptions,
) -> Comparison {
    let (base, base_dropped) = usable_runs(baseline);
    let (cand, cand_dropped) = usable_runs(candidate);
    let baseline = SampleSummary::of(&base, base_dropped);
    let candidate = SampleSummary::of(&cand, cand_dropped);
    let q = opts.quantile;
    let mut reasons = Vec::new();
    for (side, s) in [("baseline", &baseline), ("candidate", &candidate)] {
        if s.dropped > 0 {
            reasons.push(format!(
                "{side}: {} non-finite or zero-length sample(s) dropped",
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
    let few_runs = baseline.runs.min(candidate.runs) < SETTLED_RUNS;
    let done = |verdict, interval: Option<Interval>, reasons| Comparison {
        low_run_count: interval.is_some() && few_runs,
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
        .chain(
            [("baseline", baseline.runs), ("candidate", candidate.runs)]
                .into_iter()
                .filter(|(_, runs)| *runs < opts.min_runs)
                .map(|(side, runs)| {
                    format!(
                        "{side} has {runs} run(s) with samples; at least {} needed",
                        opts.min_runs
                    )
                }),
        )
        .collect::<Vec<_>>();
    // An empty side can never be resampled, whatever `min_samples` says.
    let empty: Vec<String> = [
        ("baseline", base.is_empty()),
        ("candidate", cand.is_empty()),
    ]
    .into_iter()
    .filter(|(_, is_empty)| *is_empty)
    .map(|(side, _)| format!("{side} has no samples"))
    .collect();
    if !too_few.is_empty() || !empty.is_empty() || opts.iterations == 0 {
        reasons.extend(too_few);
        reasons.extend(empty);
        if opts.iterations == 0 {
            reasons.push("bootstrap iterations is 0".into());
        }
        return done(Verdict::Inconclusive, None, reasons);
    }
    let ci = stats::cluster_bootstrap_difference(
        &base,
        &cand,
        q,
        opts.confidence,
        opts.iterations,
        opts.seed,
    );
    let (verdict, reason) = classify(ci, margin, q);
    reasons.push(reason);
    if few_runs {
        reasons.push(format!(
            "the interval was drawn from {} baseline and {} candidate run(s), fewer than \
             {SETTLED_RUNS} runs on a side: read its bounds as coarse",
            base.len(),
            cand.len()
        ));
    }
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
    // Significant (zero excluded) but straddling the margin: the change is
    // real, whether it matters is not settled.
    if ci.low > 0.0 && ci.high > margin {
        return (
            Verdict::Inconclusive,
            format!(
                "{q} difference interval {band} is significant but its magnitude against the margin is unresolved"
            ),
        );
    }
    if ci.high < 0.0 && ci.low < -margin {
        return (
            Verdict::Inconclusive,
            format!(
                "{q} difference interval {band} is a significant improvement but its magnitude against the margin is unresolved"
            ),
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
    fn a_significant_shift_straddling_the_margin_is_inconclusive() {
        let base = around(100.0, 200, 1.0);
        let cand = around(106.0, 200, 1.0);
        // Put the margin inside the (deterministic) interval, so the shift
        // is clearly real but whether it exceeds the margin is not.
        let first = compare(&base, &cand, &CompareOptions::default());
        let ci = first.interval.expect("interval");
        assert!(ci.low > 0.0, "{first:?}");
        let mid = f64::midpoint(ci.low, ci.high);
        let opts = CompareOptions {
            noise_margin_pct: mid / first.baseline.p95.unwrap() * 100.0,
            ..CompareOptions::default()
        };
        let c = compare(&base, &cand, &opts);
        assert!(c.interval == Some(ci), "same seed, same interval");
        assert!(c.margin > ci.low && c.margin < ci.high, "{c:?}");
        assert_eq!(c.verdict, Verdict::Inconclusive, "{c:?}");
        assert!(
            c.reasons.last().unwrap().contains("unresolved"),
            "{:?}",
            c.reasons
        );
        // Mirrored for an improvement of the same size (the margin is
        // relative to the new baseline, so it is re-derived).
        let swapped = compare(&cand, &base, &CompareOptions::default());
        let ci = swapped.interval.expect("interval");
        let opts = CompareOptions {
            noise_margin_pct: -f64::midpoint(ci.low, ci.high) / swapped.baseline.p95.unwrap()
                * 100.0,
            ..CompareOptions::default()
        };
        let c = compare(&cand, &base, &opts);
        assert!(c.margin > -ci.high && c.margin < -ci.low, "{c:?}");
        assert_eq!(c.verdict, Verdict::Inconclusive, "{c:?}");
        assert!(c.reasons.last().unwrap().contains("improvement"));
        // The pure rule, with the reviewer's numbers.
        let (v, _) = classify(
            Interval {
                low: 4.0,
                high: 12.0,
            },
            5.0,
            Quantile::P95,
        );
        assert_eq!(v, Verdict::Inconclusive);
        let (v, _) = classify(
            Interval {
                low: -12.0,
                high: -4.0,
            },
            5.0,
            Quantile::P95,
        );
        assert_eq!(v, Verdict::Inconclusive);
        let (v, _) = classify(
            Interval {
                low: 1.0,
                high: 4.0,
            },
            5.0,
            Quantile::P95,
        );
        assert_eq!(v, Verdict::Unchanged);
    }

    #[test]
    fn an_empty_side_is_inconclusive_even_with_min_samples_zero() {
        let base = around(100.0, 30, 5.0);
        let opts = CompareOptions {
            min_samples: 0,
            ..CompareOptions::default()
        };
        let c = compare(&base, &[], &opts);
        assert_eq!(c.verdict, Verdict::Inconclusive);
        assert!(c
            .reasons
            .iter()
            .any(|r| r.contains("candidate has no samples")));
        let c = compare(&[], &[], &opts);
        assert_eq!(c.verdict, Verdict::Inconclusive);
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

    /// `runs` runs of `windows` whole windows each, run `i` around
    /// `levels[i]`.
    fn runs_at(levels: &[f64], windows: usize) -> Vec<Vec<Window>> {
        levels
            .iter()
            .map(|level| {
                around(*level, windows, 2.0)
                    .into_iter()
                    .map(Window::whole)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn too_few_runs_is_inconclusive_with_a_reason() {
        let opts = CompareOptions::default();
        assert_eq!(opts.min_runs, 3);
        let base = runs_at(&[100.0, 100.0, 100.0], 20);
        let two = runs_at(&[150.0, 150.0], 30);
        let c = compare_runs(&base, &two, &opts);
        assert_eq!(c.verdict, Verdict::Inconclusive, "{c:?}");
        assert_eq!((c.baseline.runs, c.candidate.runs), (3, 2));
        assert_eq!((c.baseline.n, c.candidate.n), (60, 60));
        assert!(c.interval.is_none());
        assert!(
            c.reasons
                .iter()
                .any(|r| r == "candidate has 2 run(s) with samples; at least 3 needed"),
            "{:?}",
            c.reasons
        );
        // A run without a usable window is not a run.
        let mut hollow = runs_at(&[150.0, 150.0], 30);
        hollow.push(vec![Window::whole(f64::NAN)]);
        hollow.push(Vec::new());
        let c = compare_runs(&base, &hollow, &opts);
        assert_eq!(c.candidate.runs, 2);
        assert_eq!(c.candidate.dropped, 1);
        assert_eq!(c.verdict, Verdict::Inconclusive);
        // Enough of them: a verdict.
        let three = runs_at(&[150.0, 150.0, 150.0], 20);
        let c = compare_runs(&base, &three, &opts);
        assert_eq!(c.verdict, Verdict::Regressed, "{c:?}");
        let lenient = CompareOptions {
            min_runs: 2,
            ..opts
        };
        assert_eq!(
            compare_runs(&base, &two, &lenient).verdict,
            Verdict::Regressed
        );
    }

    #[test]
    fn a_verdict_drawn_from_fewer_than_five_runs_a_side_says_so() {
        assert_eq!(SETTLED_RUNS, 5);
        let opts = CompareOptions::default();
        let three = |level| runs_at(&[level, level, level], 20);
        let five = |level| runs_at(&[level; 5], 20);
        // Three runs a side resample in ten distinct ways.
        let c = compare_runs(&three(100.0), &three(150.0), &opts);
        assert_eq!(c.verdict, Verdict::Regressed);
        assert!(c.low_run_count, "{c:?}");
        // The verdict says so in words too, after why it is what it is.
        let coarse = |c: &Comparison| {
            c.reasons
                .iter()
                .filter(|r| r.contains("fewer than 5 runs on a side"))
                .count()
        };
        assert_eq!(coarse(&c), 1, "{:?}", c.reasons);
        let said = c.reasons.last().unwrap();
        assert!(
            said.contains("3 baseline") && said.contains("3 candidate"),
            "{said}"
        );
        assert_eq!(coarse(&compare_runs(&five(100.0), &five(150.0), &opts)), 0);
        // One thin side is enough.
        assert!(compare_runs(&five(100.0), &three(150.0), &opts).low_run_count);
        assert!(compare_runs(&three(100.0), &five(150.0), &opts).low_run_count);
        assert!(!compare_runs(&five(100.0), &five(150.0), &opts).low_run_count);
        // No interval was drawn: nothing to warn of.
        let two = runs_at(&[150.0, 150.0], 30);
        let c = compare_runs(&three(100.0), &two, &opts);
        assert_eq!(c.verdict, Verdict::Inconclusive);
        assert!(!c.low_run_count);
        assert_eq!(coarse(&c), 0, "{:?}", c.reasons);
        // An answer from before the field reads as not flagged.
        let mut old = serde_json::to_value(&c).unwrap();
        old.as_object_mut().unwrap().remove("low_run_count");
        let old: Comparison = serde_json::from_value(old).unwrap();
        assert!(!old.low_run_count);
    }

    #[test]
    fn one_slow_run_among_several_is_not_called_a_regression() {
        // A third of the candidate's windows are slow, all of them in one
        // run. Window by window that is a p95 well above the baseline's;
        // run by run it is one run out of three.
        let base = runs_at(&[100.0, 100.0, 100.0], 40);
        let cand = runs_at(&[100.0, 100.0, 140.0], 40);
        let flat =
            |runs: &[Vec<Window>]| -> Vec<f64> { runs.iter().flatten().map(|w| w.value).collect() };
        let opts = CompareOptions::default();
        let by_window = compare(&flat(&base), &flat(&cand), &opts);
        assert_eq!(by_window.verdict, Verdict::Regressed, "{by_window:?}");
        let by_run = compare_runs(&base, &cand, &opts);
        assert_ne!(by_run.verdict, Verdict::Regressed, "{by_run:?}");
        let ci = by_run.interval.expect("interval");
        assert!(ci.low <= by_run.margin, "{by_run:?}");
        // Every run slow: a regression either way.
        let all = runs_at(&[140.0, 140.0, 140.0], 40);
        assert_eq!(compare_runs(&base, &all, &opts).verdict, Verdict::Regressed);
    }

    #[test]
    fn a_shortened_window_weighs_by_its_sampled_duration() {
        let opts = CompareOptions {
            quantile: Quantile::P99,
            ..CompareOptions::default()
        };
        let base = runs_at(&[100.0, 100.0, 100.0], 30);
        let with_tail = |weight: f64| -> Vec<Vec<Window>> {
            let mut runs = runs_at(&[100.0, 100.0, 100.0], 30);
            for run in &mut runs {
                // Two slow closing windows a run.
                run.push(Window {
                    value: 400.0,
                    weight,
                });
                run.push(Window {
                    value: 400.0,
                    weight,
                });
            }
            runs
        };
        let whole = compare_runs(&base, &with_tail(1.0), &opts);
        assert_eq!(whole.candidate.p99, Some(400.0));
        let cut = compare_runs(&base, &with_tail(0.01), &opts);
        assert!(cut.candidate.p99.unwrap() < 110.0, "{cut:?}");
        assert_eq!(cut.candidate.n, 96);
        // A window with no duration says nothing.
        let none = compare_runs(&base, &with_tail(0.0), &opts);
        assert_eq!((none.candidate.n, none.candidate.dropped), (90, 6));
    }

    #[test]
    fn the_same_runs_and_seed_give_the_same_interval() {
        let base = runs_at(
            &[100.0, 104.0, 98.0, 101.0, 96.0, 107.0, 102.0, 99.0, 105.0],
            25,
        );
        let cand = runs_at(
            &[
                108.0, 112.0, 103.0, 109.0, 115.0, 106.0, 111.0, 104.0, 118.0,
            ],
            25,
        );
        let opts = CompareOptions::default();
        let a = compare_runs(&base, &cand, &opts);
        let b = compare_runs(&base, &cand, &opts);
        assert_eq!(a, b);
        assert!(a.interval.is_some());
        let reseeded = CompareOptions { seed: 42, ..opts };
        let c = compare_runs(&base, &cand, &reseeded);
        assert_eq!(c.seed, 42);
        assert_ne!(c.interval, a.interval, "another seed draws other runs");
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
