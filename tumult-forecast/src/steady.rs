//! Steady-state check: forecast the latest window from the history before
//! it and ask whether the window sits inside its own 10–90 band. A target
//! that is already outside its band is not in a steady state, and a run
//! against it would measure the existing incident, not the fault.

use serde::Serialize;

use crate::{Forecast, ForecastError, ForecastProvider, ForecastRequest, DEFAULT_LEVELS};

/// Share of observed window steps allowed outside the band before the
/// target counts as unsteady (a 10–90 band leaves 20 % outside by
/// construction; a bit more is noise).
pub const DEFAULT_TOLERANCE: f64 = 0.25;

/// The verdict for one series.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SteadyState {
    /// Inside the band (within tolerance).
    pub steady: bool,
    /// Steps in the checked window.
    pub window: usize,
    /// Observed (non-gap) steps in the window.
    pub observed: usize,
    /// Observed steps outside the band.
    pub outside: usize,
    pub tolerance: f64,
    /// The forecast over the window.
    pub forecast: Forecast,
}

/// Check the last `window` steps of `values` against a forecast made from
/// everything before them.
///
/// # Errors
/// [`ForecastError::InvalidWindow`] when the window is empty or leaves no
/// history before it; provider errors pass through.
#[allow(clippy::cast_precision_loss)] // step counts are far below 2^52
pub fn steady_state(
    provider: &dyn ForecastProvider,
    values: &[f64],
    window: usize,
    tolerance: f64,
) -> Result<SteadyState, ForecastError> {
    if window == 0 {
        return Err(ForecastError::InvalidWindow(
            "the steady-state window must be at least one step".into(),
        ));
    }
    if window >= values.len() {
        return Err(ForecastError::InvalidWindow(format!(
            "a {window}-step window leaves no history in {} steps",
            values.len()
        )));
    }
    let split = values.len() - window;
    let forecast = provider.forecast(&ForecastRequest {
        context: &values[..split],
        horizon: window,
        levels: &DEFAULT_LEVELS,
        covariates: None,
    })?;
    let (lower, upper) = forecast
        .band(0.1, 0.9)
        .ok_or_else(|| ForecastError::Provider("forecast lacks the 10-90 band".into()))?;
    let recent = &values[split..];
    let observed = recent.iter().filter(|v| v.is_finite()).count();
    let outside = recent
        .iter()
        .enumerate()
        .filter(|(i, v)| v.is_finite() && (**v < lower[*i] || **v > upper[*i]))
        .count();
    let steady = observed > 0 && outside as f64 <= tolerance * observed as f64;
    Ok(SteadyState {
        steady,
        window,
        observed,
        outside,
        tolerance,
        forecast,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HoltWinters;

    #[allow(clippy::cast_precision_loss)]
    fn noisy(n: usize) -> Vec<f64> {
        (0..n).map(|i| 50.0 + ((i * 7) % 5) as f64).collect()
    }

    #[test]
    fn a_quiet_tail_is_steady_and_a_spike_is_not() {
        let values = noisy(60);
        let ok = steady_state(&HoltWinters::new(), &values, 10, DEFAULT_TOLERANCE).unwrap();
        assert!(ok.steady, "{ok:?}");
        assert_eq!(ok.window, 10);
        assert_eq!(ok.observed, 10);
        assert_eq!(ok.forecast.horizon(), 10);

        let mut spiked = values;
        for v in spiked.iter_mut().rev().take(6) {
            *v = 500.0;
        }
        let bad = steady_state(&HoltWinters::new(), &spiked, 10, DEFAULT_TOLERANCE).unwrap();
        assert!(!bad.steady);
        assert!(bad.outside >= 6);
    }

    #[test]
    fn gaps_do_not_count_and_an_all_gap_window_is_not_steady() {
        let mut values = noisy(40);
        for v in values.iter_mut().rev().take(5) {
            *v = f64::NAN;
        }
        let s = steady_state(&HoltWinters::new(), &values, 5, DEFAULT_TOLERANCE).unwrap();
        assert_eq!(s.observed, 0);
        assert!(!s.steady);
    }

    #[test]
    fn window_must_fit() {
        let values = noisy(10);
        assert!(matches!(
            steady_state(&HoltWinters::new(), &values, 0, 0.2),
            Err(ForecastError::InvalidWindow(_))
        ));
        assert!(matches!(
            steady_state(&HoltWinters::new(), &values, 10, 0.2),
            Err(ForecastError::InvalidWindow(_))
        ));
    }
}
