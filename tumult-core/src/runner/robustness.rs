//! Robustness metrics, judged from the windows of a load: how far a soak
//! drifted, and how long the target took to come back after a spike.
//!
//! | metric | what it is |
//! |---|---|
//! | `load.p95_drift_pct` | the median p95 of the last quarter of the windows against that of the first quarter, in percent |
//! | `load.error_rate_drift` | the median error rate of the last quarter less that of the first quarter |
//! | `load.spike_recovery_s` | the seconds from the end of the spike to the first window whose p95 is back in the band before it |
//!
//! The windows are the [`LoadSample`]s the load adapter kept: oha keeps
//! one per window in which anything was answered, k6 keeps none. A window
//! in which nothing was answered leaves no sample, so a drift does not
//! see it. Too few windows to judge gives no value, so the objective is
//! inconclusive and says why.

use crate::types::{LoadConfig, LoadProfile, LoadResult, LoadSample};

pub(crate) const P95_DRIFT_PCT: &str = "load.p95_drift_pct";
pub(crate) const ERROR_RATE_DRIFT: &str = "load.error_rate_drift";
pub(crate) const SPIKE_RECOVERY_S: &str = "load.spike_recovery_s";

/// The fewest windows a drift is judged on: one in each quarter.
const DRIFT_MIN_WINDOWS: usize = 4;

/// A window after the spike is back when its p95 is at most this many
/// times the median p95 of the windows before the spike.
const SPIKE_BAND: f64 = 1.2;

/// What the windows say about a robustness metric.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Judged {
    /// The value the windows show.
    Value(f64),
    /// No window after the spike was back in the band: the seconds from
    /// the end of the spike to the end of the last window, a lower bound
    /// of the time the target takes to come back.
    NotBack(f64),
    /// The load kept too few windows to judge.
    TooFewWindows,
}

/// Judge `metric` from the windows of `load`, run with `profile`. `None`
/// when `metric` is not a robustness metric. Pure.
pub(crate) fn judge(
    metric: &str,
    profile: Option<&LoadProfile>,
    load: Option<&LoadResult>,
) -> Option<Judged> {
    let windows = load.map_or(&[][..], |l| l.samples.as_slice());
    match metric {
        P95_DRIFT_PCT => Some(p95_drift_pct(windows)),
        ERROR_RATE_DRIFT => Some(error_rate_drift(windows)),
        SPIKE_RECOVERY_S => Some(spike_recovery_s(profile, load)),
        _ => None,
    }
}

/// Why no run of `load` can measure `metric`, or `None` when it can or
/// `metric` is not a robustness metric: a drift needs a soak, and the
/// recovery after a spike needs a spike cut into windows. Pure.
pub(crate) fn profile_fault(metric: &str, load: Option<&LoadConfig>) -> Option<String> {
    let needs = match metric {
        P95_DRIFT_PCT | ERROR_RATE_DRIFT => "soak",
        SPIKE_RECOVERY_S => "spike",
        _ => return None,
    };
    let profile = load.and_then(|l| l.profile.as_ref());
    match profile {
        Some(LoadProfile::Soak { .. }) if needs == "soak" => None,
        Some(LoadProfile::Spike {
            sample_interval_s: Some(_),
            ..
        }) if needs == "spike" => None,
        Some(LoadProfile::Spike { .. }) if needs == "spike" => Some(format!(
            "metric '{metric}' needs a spike profile with sample_interval_s, so that the \
             windows after the spike show when the target is back"
        )),
        _ => Some(format!(
            "metric '{metric}' needs a load with a {needs} profile, and {}",
            what_runs(load)
        )),
    }
}

/// The load of an experiment, in words.
fn what_runs(load: Option<&LoadConfig>) -> &'static str {
    match load.map(|l| l.profile.as_ref()) {
        None => "this experiment has no load",
        Some(None) => "this load has no profile",
        Some(Some(LoadProfile::Ramp { .. })) => "this load has a ramp profile",
        Some(Some(LoadProfile::Spike { .. })) => "this load has a spike profile",
        Some(Some(LoadProfile::Soak { .. })) => "this load has a soak profile",
    }
}

/// How much the p95 grew from the first quarter of the windows to the
/// last, in percent of the first. Only a window that measured a latency
/// above zero counts.
fn p95_drift_pct(windows: &[LoadSample]) -> Judged {
    let p95: Vec<f64> = windows
        .iter()
        .map(|w| w.latency_p95_ms)
        .filter(|ms| *ms > 0.0)
        .collect();
    match quarter_medians(&p95) {
        Some((first, last)) => Judged::Value((last - first) * 100.0 / first),
        None => Judged::TooFewWindows,
    }
}

/// How much the error rate rose from the first quarter of the windows to
/// the last, as a share: 0.02 is two percentage points.
fn error_rate_drift(windows: &[LoadSample]) -> Judged {
    let rates: Vec<f64> = windows.iter().map(|w| w.error_rate).collect();
    match quarter_medians(&rates) {
        Some((first, last)) => Judged::Value(last - first),
        None => Judged::TooFewWindows,
    }
}

/// The medians of the first and the last quarter of `series`, which is in
/// time order; `None` when it has fewer than [`DRIFT_MIN_WINDOWS`] values.
fn quarter_medians(series: &[f64]) -> Option<(f64, f64)> {
    if series.len() < DRIFT_MIN_WINDOWS {
        return None;
    }
    let quarter = series.len() / 4;
    let first = median(&series[..quarter]);
    let last = median(&series[series.len() - quarter..]);
    Some((first, last))
}

/// The median of `values`, which is not empty.
fn median(values: &[f64]) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let middle = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        return f64::midpoint(sorted[middle - 1], sorted[middle]);
    }
    sorted[middle]
}

/// The seconds from the end of the spike to the start of the first window
/// after it whose p95 is back in the band. The spike starts `after_s` and
/// ends `after_s + hold_s` into the load, as its plan says, so a window
/// is before the spike when it started before the spike did, and after it
/// when it started once the spike was over.
fn spike_recovery_s(profile: Option<&LoadProfile>, load: Option<&LoadResult>) -> Judged {
    let (
        Some(LoadProfile::Spike {
            after_s, hold_s, ..
        }),
        Some(load),
    ) = (profile, load)
    else {
        return Judged::TooFewWindows;
    };
    let spike_starts_ns = load.started_at_ns + nanos(*after_s);
    let spike_ends_ns = load.started_at_ns + nanos(after_s + hold_s);
    let before: Vec<f64> = load
        .samples
        .iter()
        .filter(|w| w.ts_ns < spike_starts_ns)
        .map(|w| w.latency_p95_ms)
        .collect();
    let after: Vec<&LoadSample> = load
        .samples
        .iter()
        .filter(|w| w.ts_ns >= spike_ends_ns)
        .collect();
    let Some(last) = after.last() else {
        return Judged::TooFewWindows;
    };
    if before.is_empty() {
        return Judged::TooFewWindows;
    }
    let band_ms = SPIKE_BAND * median(&before);
    if let Some(back) = after.iter().find(|w| w.latency_p95_ms <= band_ms) {
        return Judged::Value(seconds(back.ts_ns - spike_ends_ns));
    }
    Judged::NotBack(seconds(last.ts_ns - spike_ends_ns) + last.duration_s)
}

/// Seconds of a plan as nanoseconds.
fn nanos(seconds: f64) -> i64 {
    // A validated plan lasts far less than the 292 years an i64 holds.
    #[allow(clippy::cast_possible_truncation)]
    let ns = (seconds * 1_000_000_000.0).round() as i64;
    ns
}

/// Nanoseconds as seconds.
fn seconds(ns: i64) -> f64 {
    // i64 → f64 precision loss acceptable for a duration.
    #[allow(clippy::cast_precision_loss)]
    let s = ns as f64 / 1_000_000_000.0;
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{LoadExecution, LoadTool};
    use std::collections::HashMap;

    const SECOND_NS: i64 = 1_000_000_000;

    /// A window `at_s` seconds into the load, one second long.
    fn window(at_s: i64, p95_ms: f64, error_rate: f64) -> LoadSample {
        LoadSample {
            ts_ns: at_s * SECOND_NS,
            duration_s: 1.0,
            vus: 2,
            throughput_rps: 50.0,
            latency_p50_ms: p95_ms / 2.0,
            latency_p95_ms: p95_ms,
            latency_p99_ms: p95_ms,
            error_rate,
            requests: 50,
            counts: None,
            request_timeout_ms: None,
        }
    }

    /// A load that started at 0 and kept `samples`.
    fn kept(samples: Vec<LoadSample>) -> LoadResult {
        LoadResult {
            tool: LoadTool::Oha,
            started_at_ns: 0,
            ended_at_ns: 60 * SECOND_NS,
            duration_s: 60.0,
            vus: 2,
            throughput_rps: 50.0,
            latency_p50_ms: 50.0,
            latency_p95_ms: 100.0,
            latency_p99_ms: 100.0,
            error_rate: 0.0,
            total_requests: 50,
            thresholds_met: true,
            samples,
            execution: LoadExecution::default(),
        }
    }

    /// One window a second with these p95s, without errors.
    fn series(p95_ms: &[f64]) -> Vec<LoadSample> {
        (0_i64..)
            .zip(p95_ms)
            .map(|(at_s, p95)| window(at_s, *p95, 0.0))
            .collect()
    }

    fn drift(metric: &str, samples: Vec<LoadSample>) -> Judged {
        judge(metric, None, Some(&kept(samples))).expect("a robustness metric")
    }

    #[test]
    fn a_drift_compares_the_medians_of_the_first_and_the_last_quarter() {
        // Eight windows: quarters of two, medians 100 and 160.
        let p95 = [90.0, 110.0, 400.0, 10.0, 130.0, 140.0, 150.0, 170.0];

        assert_eq!(drift(P95_DRIFT_PCT, series(&p95)), Judged::Value(60.0));
    }

    #[test]
    fn a_quarter_is_the_whole_windows_of_a_fourth_of_the_series() {
        // Five windows: quarters of one, the middle three left out.
        let p95 = [100.0, 900.0, 900.0, 900.0, 80.0];

        assert_eq!(drift(P95_DRIFT_PCT, series(&p95)), Judged::Value(-20.0));
    }

    #[test]
    fn a_window_without_a_latency_does_not_count_for_the_p95_drift() {
        let p95 = [100.0, 0.0, 100.0, 100.0];

        assert_eq!(drift(P95_DRIFT_PCT, series(&p95)), Judged::TooFewWindows);
    }

    #[test]
    fn the_error_rate_drift_is_the_rise_of_the_median_share() {
        let samples = vec![
            window(0, 100.0, 0.01),
            window(1, 100.0, 0.02),
            window(2, 100.0, 0.30),
            window(3, 100.0, 0.05),
        ];

        let Judged::Value(rose) = drift(ERROR_RATE_DRIFT, samples) else {
            panic!("a drift");
        };
        assert!((rose - 0.04).abs() < 1e-12, "{rose}");
    }

    #[test]
    fn fewer_than_four_windows_or_no_load_are_too_few_to_judge_a_drift() {
        for metric in [P95_DRIFT_PCT, ERROR_RATE_DRIFT] {
            assert_eq!(
                drift(metric, series(&[100.0, 100.0, 100.0])),
                Judged::TooFewWindows
            );
            assert_eq!(judge(metric, None, None), Some(Judged::TooFewWindows));
        }
    }

    #[test]
    fn a_metric_that_is_not_a_robustness_metric_is_not_judged_here() {
        assert_eq!(judge("load.latency_p95_ms", None, None), None);
    }

    /// Two users, twenty for two seconds after three.
    fn spike_profile() -> LoadProfile {
        LoadProfile::Spike {
            peak_vus: 20,
            after_s: 3.0,
            hold_s: 2.0,
            sample_interval_s: Some(1.0),
        }
    }

    fn recovery(samples: Vec<LoadSample>) -> Judged {
        judge(
            SPIKE_RECOVERY_S,
            Some(&spike_profile()),
            Some(&kept(samples)),
        )
        .expect("a robustness metric")
    }

    #[test]
    fn the_spike_recovery_is_the_time_to_the_first_window_back_in_the_band() {
        // Before: median 100 ms, so the band is up to 120 ms. The spike
        // ends 5 s in; the window at 7 s is the first back.
        let p95 = [90.0, 100.0, 110.0, 900.0, 900.0, 500.0, 121.0, 120.0, 300.0];

        assert_eq!(recovery(series(&p95)), Judged::Value(2.0));
    }

    #[test]
    fn a_window_at_the_end_of_the_spike_that_is_back_is_a_recovery_of_zero() {
        let p95 = [100.0, 100.0, 100.0, 900.0, 900.0, 100.0];

        assert_eq!(recovery(series(&p95)), Judged::Value(0.0));
    }

    #[test]
    fn a_window_during_the_spike_is_neither_before_nor_after_it() {
        // The windows at 3 and 4 s are fast, yet the spike has not ended.
        let p95 = [100.0, 100.0, 100.0, 50.0, 50.0, 300.0, 100.0];

        assert_eq!(recovery(series(&p95)), Judged::Value(1.0));
    }

    #[test]
    fn a_target_that_is_not_back_when_the_load_ends_gives_the_time_it_watched() {
        let p95 = [100.0, 100.0, 100.0, 900.0, 900.0, 500.0, 400.0, 300.0];

        // From the end of the spike at 5 s to the end of the window at 7 s.
        assert_eq!(recovery(series(&p95)), Judged::NotBack(3.0));
    }

    #[test]
    fn a_spike_with_no_window_before_or_after_it_is_too_few_to_judge() {
        let no_window_before = vec![window(3, 900.0, 0.0), window(5, 100.0, 0.0)];
        assert_eq!(recovery(no_window_before), Judged::TooFewWindows);
        let no_window_after = series(&[100.0, 100.0, 100.0, 900.0, 900.0]);
        assert_eq!(recovery(no_window_after), Judged::TooFewWindows);
        let soak = LoadProfile::Soak {
            sample_interval_s: 1.0,
        };
        let windows = kept(series(&[100.0; 8]));
        assert_eq!(
            judge(SPIKE_RECOVERY_S, Some(&soak), Some(&windows)),
            Some(Judged::TooFewWindows)
        );
    }

    fn with_profile(profile: Option<LoadProfile>) -> LoadConfig {
        LoadConfig {
            tool: LoadTool::Oha,
            script: "http://127.0.0.1:8080/".into(),
            vus: Some(2),
            duration_s: Some(60.0),
            thresholds: HashMap::new(),
            profile,
        }
    }

    #[test]
    fn a_drift_needs_a_soak_and_says_which_profile_it_has() {
        let soak = with_profile(Some(LoadProfile::Soak {
            sample_interval_s: 10.0,
        }));
        assert_eq!(profile_fault(P95_DRIFT_PCT, Some(&soak)), None);
        assert_eq!(profile_fault(ERROR_RATE_DRIFT, Some(&soak)), None);

        let spike = with_profile(Some(spike_profile()));
        assert_eq!(
            profile_fault(P95_DRIFT_PCT, Some(&spike)).as_deref(),
            Some(
                "metric 'load.p95_drift_pct' needs a load with a soak profile, \
                 and this load has a spike profile"
            )
        );
        let flat = with_profile(None);
        let fault = profile_fault(ERROR_RATE_DRIFT, Some(&flat)).expect("a fault");
        assert!(fault.ends_with("and this load has no profile"), "{fault}");
        let fault = profile_fault(P95_DRIFT_PCT, None).expect("a fault");
        assert!(
            fault.ends_with("and this experiment has no load"),
            "{fault}"
        );
    }

    #[test]
    fn the_spike_recovery_needs_a_spike_cut_into_windows() {
        let spike = with_profile(Some(spike_profile()));
        assert_eq!(profile_fault(SPIKE_RECOVERY_S, Some(&spike)), None);

        let whole = with_profile(Some(LoadProfile::Spike {
            peak_vus: 20,
            after_s: 3.0,
            hold_s: 2.0,
            sample_interval_s: None,
        }));
        let fault = profile_fault(SPIKE_RECOVERY_S, Some(&whole)).expect("a fault");
        assert!(
            fault.contains("needs a spike profile with sample_interval_s"),
            "{fault}"
        );

        let soak = with_profile(Some(LoadProfile::Soak {
            sample_interval_s: 10.0,
        }));
        assert_eq!(
            profile_fault(SPIKE_RECOVERY_S, Some(&soak)).as_deref(),
            Some(
                "metric 'load.spike_recovery_s' needs a load with a spike profile, \
                 and this load has a soak profile"
            )
        );

        let ramp = with_profile(Some(LoadProfile::Ramp {
            start_vus: 1,
            ramp_up_s: 10.0,
        }));
        assert_eq!(
            profile_fault(SPIKE_RECOVERY_S, Some(&ramp)).as_deref(),
            Some(
                "metric 'load.spike_recovery_s' needs a load with a spike profile, \
                 and this load has a ramp profile"
            )
        );
    }

    #[test]
    fn any_other_metric_needs_no_profile() {
        assert_eq!(profile_fault("load.latency_p95_ms", None), None);
    }
}
