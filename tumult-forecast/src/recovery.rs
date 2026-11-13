//! Recovery time: the first step after the fault ended at which the
//! observed series re-entered the forecast band and stayed inside it for
//! `stable_steps` observed steps. The same function gives measured RTO for
//! disaster-recovery runs and MTTR for chaos runs; only the series differs.

use serde::Serialize;

/// Observed steps in a row the series must stay inside the band before it
/// counts as recovered.
pub const DEFAULT_STABLE_STEPS: usize = 3;

/// The recovery verdict for one series. Indices refer to the arrays the
/// caller passed in (the forecast horizon, in the counterfactual's case).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Recovery {
    /// Step at which the stable in-band run began; `None` when the series
    /// did not settle inside the band before the arrays ran out.
    pub recovered_at: Option<usize>,
    /// Steps between the fault end and `recovered_at`.
    pub steps_after_fault: Option<usize>,
    /// The `stable_steps` that were required.
    pub stable_steps: usize,
    /// Observed steps outside the band during the fault window.
    pub outside_during_fault: usize,
    /// Observed steps outside the band after the fault end.
    pub outside_after_fault: usize,
}

impl Recovery {
    /// `steps_after_fault` in seconds at the series' step.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // steps and nanoseconds are far below 2^52
    pub fn seconds(&self, step_ns: i64) -> Option<f64> {
        self.steps_after_fault
            .map(|steps| steps as f64 * step_ns as f64 / 1e9)
    }
}

fn inside(observed: f64, lower: f64, upper: f64) -> Option<bool> {
    if !observed.is_finite() {
        return None;
    }
    Some(observed >= lower && observed <= upper)
}

/// Find the recovery step. `fault_end` is the first step after the fault
/// (clamped to the arrays); gaps (`NaN`) neither count towards the stable
/// run nor break it. A `stable_steps` of zero means one.
#[must_use]
pub fn recovery_time(
    observed: &[f64],
    lower: &[f64],
    upper: &[f64],
    fault_end: usize,
    stable_steps: usize,
) -> Recovery {
    let n = observed.len().min(lower.len()).min(upper.len());
    let stable_steps = stable_steps.max(1);
    let fault_end = fault_end.min(n);
    let status = |i: usize| inside(observed[i], lower[i], upper[i]);
    let outside_during_fault = (0..fault_end).filter(|i| status(*i) == Some(false)).count();
    let outside_after_fault = (fault_end..n).filter(|i| status(*i) == Some(false)).count();
    let mut run_start: Option<usize> = None;
    let mut run_len = 0;
    let mut recovered_at = None;
    for i in fault_end..n {
        match status(i) {
            Some(true) => {
                run_start.get_or_insert(i);
                run_len += 1;
                if run_len >= stable_steps {
                    recovered_at = run_start;
                    break;
                }
            }
            Some(false) => {
                run_start = None;
                run_len = 0;
            }
            None => {}
        }
    }
    Recovery {
        recovered_at,
        steps_after_fault: recovered_at.map(|at| at - fault_end),
        stable_steps,
        outside_during_fault,
        outside_after_fault,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LO: [f64; 10] = [0.0; 10];
    const HI: [f64; 10] = [1.0; 10];

    #[test]
    fn recovers_at_the_first_stable_run() {
        // Fault over steps 0..4; observed leaves the band, comes back at 6
        // but dips out at 7, then settles from 8.
        let observed = [0.5, 5.0, 5.0, 5.0, 5.0, 5.0, 0.5, 2.0, 0.5, 0.5];
        let r = recovery_time(&observed, &LO, &HI, 4, 2);
        assert_eq!(r.recovered_at, Some(8));
        assert_eq!(r.steps_after_fault, Some(4));
        assert_eq!(r.outside_during_fault, 3);
        assert_eq!(r.outside_after_fault, 3);
        assert_eq!(r.seconds(1_000_000_000), Some(4.0));
        assert_eq!(r.stable_steps, 2);
    }

    #[test]
    fn immediate_recovery_when_never_outside_after_the_fault() {
        let observed = [0.5, 5.0, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5];
        let r = recovery_time(&observed, &LO, &HI, 2, DEFAULT_STABLE_STEPS);
        assert_eq!(r.recovered_at, Some(2));
        assert_eq!(r.steps_after_fault, Some(0));
        assert_eq!(r.outside_during_fault, 1);
    }

    #[test]
    fn no_recovery_when_the_run_is_too_short_or_never_settles() {
        let observed = [0.5, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 0.5];
        let r = recovery_time(&observed, &LO, &HI, 1, 3);
        assert_eq!(r.recovered_at, None);
        assert_eq!(r.steps_after_fault, None);
        assert_eq!(r.seconds(1), None);
        let never = [5.0; 10];
        assert_eq!(recovery_time(&never, &LO, &HI, 1, 1).recovered_at, None);
    }

    #[test]
    fn gaps_neither_count_nor_reset() {
        let observed = [5.0, 5.0, 0.5, f64::NAN, f64::NAN, 0.5, 0.5, 0.5, 0.5, 0.5];
        let r = recovery_time(&observed, &LO, &HI, 2, 2);
        assert_eq!(r.recovered_at, Some(2));
        assert_eq!(r.outside_after_fault, 0);
    }

    #[test]
    fn fault_end_past_the_arrays_and_zero_stable_steps_are_tolerated() {
        let observed = [0.5; 10];
        let r = recovery_time(&observed, &LO, &HI, 50, 0);
        assert_eq!(r.recovered_at, None);
        assert_eq!(r.stable_steps, 1);
        let r = recovery_time(&observed, &LO, &HI, 9, 0);
        assert_eq!(r.recovered_at, Some(9));
    }
}
