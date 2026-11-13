//! The built-in provider: Holt's additive-trend exponential smoothing,
//! upgraded to additive Holt-Winters when the context is long enough to
//! show a season. Pure, closed-form, deterministic.
//!
//! The smoothing parameters come from a small grid search that minimises
//! the one-step-ahead squared error, so the same context always yields the
//! same fit. The band is empirical: the quantiles of the one-step residuals,
//! centred on the point forecast and widened with the square root of the
//! lead time. Gaps (`NaN`) are linearly interpolated for the fit and do not
//! count towards the history.
//!
//! A constant or exactly periodic context leaves no residuals, so that band
//! would be a line. It is widened to the context's [`crate::band::floor`]
//! and the forecast is marked [`Forecast::degenerate_band`].
//!
//! Short contexts — fewer than [`MIN_HISTORY`] observed points — get a flat
//! forecast at the mean with a band from the few residuals there are, and
//! [`Forecast::insufficient_history`] set. The band is still drawn, but
//! nobody should read much into it.

use crate::MIN_HISTORY;
use crate::{band, stats};
use crate::{validate, Forecast, ForecastError, ForecastProvider, ForecastRequest, Provenance};

/// Longest season the auto-detector considers, in steps.
const MAX_PERIOD: usize = 512;
/// Autocorrelation a lag must reach to count as a season.
const SEASON_ACF: f64 = 0.5;
const ALPHAS: [f64; 5] = [0.1, 0.3, 0.5, 0.7, 0.9];
const BETAS: [f64; 4] = [0.05, 0.1, 0.2, 0.3];
const GAMMAS: [f64; 3] = [0.1, 0.3, 0.5];

/// How the provider treats seasonality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Seasonality {
    /// Detect a period from the autocorrelation when the context holds at
    /// least three of them.
    #[default]
    Auto,
    /// Trend only.
    Off,
    /// A known period in steps (used when the context holds at least two).
    Period(usize),
}

/// The Holt-Winters provider. See the module docs.
#[derive(Debug, Clone, Copy)]
pub struct HoltWinters {
    seasonality: Seasonality,
    min_history: usize,
}

impl Default for HoltWinters {
    fn default() -> Self {
        Self {
            seasonality: Seasonality::Auto,
            min_history: MIN_HISTORY,
        }
    }
}

impl HoltWinters {
    /// Auto-detected seasonality, the default [`MIN_HISTORY`].
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Override the seasonality handling.
    #[must_use]
    pub fn with_seasonality(mut self, seasonality: Seasonality) -> Self {
        self.seasonality = seasonality;
        self
    }

    /// The provenance every forecast from this provider carries.
    #[must_use]
    pub fn provenance() -> Provenance {
        Provenance {
            model: "holt-winters".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            weights_hash: "none".into(),
        }
    }

    fn period_for(&self, values: &[f64]) -> Option<usize> {
        match self.seasonality {
            Seasonality::Off => None,
            Seasonality::Auto => detect_period(values),
            Seasonality::Period(p) => (p >= 2 && values.len() >= 2 * p).then_some(p),
        }
    }
}

impl ForecastProvider for HoltWinters {
    fn forecast(&self, request: &ForecastRequest<'_>) -> Result<Forecast, ForecastError> {
        validate(request)?;
        let context_points = request.context.iter().filter(|v| v.is_finite()).count();
        let values = fill_gaps(request.context);
        let fit = if context_points < self.min_history {
            Fit::flat(&values)
        } else {
            best_fit(&values, self.period_for(&values))
        };
        let mut forecast = Forecast {
            levels: request.levels.to_vec(),
            quantiles: fit.quantiles(request.horizon, request.levels),
            context_points,
            insufficient_history: context_points < self.min_history,
            degenerate_band: false,
            provenance: Self::provenance(),
        };
        forecast.apply_band_floor(band::floor(request.context));
        Ok(forecast)
    }

    fn model(&self) -> &'static str {
        "holt-winters"
    }
}

/// A fitted smoothing state ready to extrapolate.
struct Fit {
    level: f64,
    trend: f64,
    /// Seasonal components indexed by `t % period`; empty when not seasonal.
    seasonal: Vec<f64>,
    /// Position of the first forecast step in the seasonal cycle.
    next_index: usize,
    residuals: Vec<f64>,
}

impl Fit {
    /// A flat forecast at the mean for short contexts.
    fn flat(values: &[f64]) -> Self {
        let mean = stats::mean(values);
        Self {
            level: mean,
            trend: 0.0,
            seasonal: Vec::new(),
            next_index: 0,
            residuals: values.iter().map(|v| v - mean).collect(),
        }
    }

    fn sse(&self) -> f64 {
        self.residuals.iter().map(|r| r * r).sum()
    }

    fn point(&self, h: usize) -> f64 {
        let seasonal = if self.seasonal.is_empty() {
            0.0
        } else {
            self.seasonal[(self.next_index + h - 1) % self.seasonal.len()]
        };
        self.level + lead(h) * self.trend + seasonal
    }

    /// `quantiles[step][level]`: the point forecast plus the residual
    /// quantile offsets, widened by `sqrt(h)`.
    fn quantiles(&self, horizon: usize, levels: &[f64]) -> Vec<Vec<f64>> {
        let centre = stats::quantile(&self.residuals, 0.5);
        let offsets: Vec<f64> = levels
            .iter()
            .map(|l| stats::quantile(&self.residuals, *l) - centre)
            .collect();
        (1..=horizon)
            .map(|h| {
                let point = self.point(h);
                let widen = lead(h).sqrt();
                offsets.iter().map(|o| point + o * widen).collect()
            })
            .collect()
    }
}

#[allow(clippy::cast_precision_loss)] // horizons are far below 2^52
fn lead(h: usize) -> f64 {
    h as f64
}

/// Linear interpolation across gaps; leading and trailing gaps take the
/// nearest observed value. The caller guarantees at least one finite point.
fn fill_gaps(context: &[f64]) -> Vec<f64> {
    let mut out = context.to_vec();
    let first = out.iter().position(|v| v.is_finite());
    let Some(first) = first else {
        return out;
    };
    for v in &mut out[..first] {
        *v = context[first];
    }
    let mut last = first;
    for i in first + 1..out.len() {
        if !out[i].is_finite() {
            continue;
        }
        let gap = i - last;
        if gap > 1 {
            let (a, b) = (out[last], out[i]);
            for (k, v) in out[last + 1..i].iter_mut().enumerate() {
                *v = a + (b - a) * lead(k + 1) / lead(gap);
            }
        }
        last = i;
    }
    let tail = out[last];
    for v in &mut out[last + 1..] {
        *v = tail;
    }
    out
}

/// The lag with the strongest autocorrelation above [`SEASON_ACF`] that is
/// also a local maximum, when the context holds at least three periods.
fn detect_period(values: &[f64]) -> Option<usize> {
    let n = values.len();
    let max_lag = (n / 3).min(MAX_PERIOD);
    if max_lag < 2 {
        return None;
    }
    let mean = stats::mean(values);
    let var: f64 = values.iter().map(|v| (v - mean) * (v - mean)).sum();
    if var <= f64::EPSILON {
        return None;
    }
    let acf = |lag: usize| -> f64 {
        values[..n - lag]
            .iter()
            .zip(&values[lag..])
            .map(|(a, b)| (a - mean) * (b - mean))
            .sum::<f64>()
            / var
    };
    let acfs: Vec<f64> = (0..=max_lag + 1).map(acf).collect();
    let mut best: Option<(usize, f64)> = None;
    for lag in 2..=max_lag {
        let here = acfs[lag];
        let is_peak = here >= acfs[lag - 1] && here >= acfs[lag + 1];
        if here > SEASON_ACF && is_peak && best.is_none_or(|(_, b)| here > b) {
            best = Some((lag, here));
        }
    }
    best.map(|(lag, _)| lag)
}

/// Grid search over the smoothing parameters; the lowest one-step SSE wins.
/// Ties keep the first (grid order), so the search is deterministic.
fn best_fit(values: &[f64], period: Option<usize>) -> Fit {
    let mut best: Option<Fit> = None;
    let mut consider = |candidate: Fit| {
        let better = best.as_ref().is_none_or(|b| candidate.sse() < b.sse());
        if better {
            best = Some(candidate);
        }
    };
    for alpha in ALPHAS {
        for beta in BETAS {
            match period {
                Some(m) => {
                    for gamma in GAMMAS {
                        consider(fit_seasonal(values, m, alpha, beta, gamma));
                    }
                }
                None => consider(fit_trend(values, alpha, beta)),
            }
        }
    }
    best.unwrap_or_else(|| Fit::flat(values))
}

/// Holt's linear (additive trend) smoothing.
fn fit_trend(y: &[f64], alpha: f64, beta: f64) -> Fit {
    let mut level = y[0];
    let mut trend = y.get(1).map_or(0.0, |y1| y1 - y[0]);
    let mut residuals = Vec::with_capacity(y.len());
    for &obs in &y[1..] {
        let predicted = level + trend;
        residuals.push(obs - predicted);
        let next_level = alpha * obs + (1.0 - alpha) * predicted;
        trend = beta * (next_level - level) + (1.0 - beta) * trend;
        level = next_level;
    }
    if residuals.is_empty() {
        residuals.push(0.0);
    }
    Fit {
        level,
        trend,
        seasonal: Vec::new(),
        next_index: 0,
        residuals,
    }
}

/// Additive Holt-Winters with period `m` (`y.len() >= 2 * m`).
fn fit_seasonal(y: &[f64], m: usize, alpha: f64, beta: f64, gamma: f64) -> Fit {
    let first = stats::mean(&y[..m]);
    let second = stats::mean(&y[m..2 * m]);
    let mut level = first;
    let mut trend = (second - first) / lead(m);
    let mut seasonal: Vec<f64> = y[..m].iter().map(|v| v - first).collect();
    let mut residuals = Vec::with_capacity(y.len());
    for (t, &obs) in y.iter().enumerate().skip(m) {
        let s = t % m;
        let predicted = level + trend + seasonal[s];
        residuals.push(obs - predicted);
        let next_level = alpha * (obs - seasonal[s]) + (1.0 - alpha) * (level + trend);
        trend = beta * (next_level - level) + (1.0 - beta) * trend;
        seasonal[s] = gamma * (obs - next_level) + (1.0 - gamma) * seasonal[s];
        level = next_level;
    }
    Fit {
        level,
        trend,
        seasonal,
        next_index: y.len() % m,
        residuals,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    fn run(context: &[f64], horizon: usize) -> Forecast {
        HoltWinters::new()
            .forecast(&ForecastRequest::new(context, horizon))
            .unwrap()
    }

    #[test]
    fn linear_trend_is_continued_with_ordered_widening_bands() {
        let context: Vec<f64> = (0..40).map(|i| 10.0 + 0.5 * lead(i)).collect();
        let f = run(&context, 5);
        assert_eq!(f.horizon(), 5);
        assert!(!f.insufficient_history);
        assert_eq!(f.context_points, 40);
        let median = f.median();
        for (h, m) in median.iter().enumerate() {
            let expected = 10.0 + 0.5 * lead(40 + h);
            assert!(close(*m, expected, 0.5), "step {h}: {m} vs {expected}");
        }
        let (lo, hi) = f.band(0.1, 0.9).unwrap();
        for h in 0..5 {
            assert!(lo[h] <= median[h] && median[h] <= hi[h]);
        }
        assert!(hi[4] - lo[4] >= hi[0] - lo[0], "band widens with lead time");
        assert_eq!(f.provenance.model, "holt-winters");
        assert_eq!(f.provenance.weights_hash, "none");
    }

    #[test]
    fn seasonal_context_is_detected_and_followed() {
        let period = 12;
        let context: Vec<f64> = (0..72)
            .map(|i| 100.0 + 20.0 * (lead(i) * std::f64::consts::TAU / lead(period)).sin())
            .collect();
        assert_eq!(detect_period(&context), Some(period));
        let f = run(&context, period);
        let median = f.median();
        for (h, m) in median.iter().enumerate() {
            let expected =
                100.0 + 20.0 * (lead(72 + h) * std::f64::consts::TAU / lead(period)).sin();
            assert!(close(*m, expected, 6.0), "step {h}: {m} vs {expected}");
        }
        let off = HoltWinters::new()
            .with_seasonality(Seasonality::Off)
            .forecast(&ForecastRequest::new(&context, period))
            .unwrap();
        assert_ne!(off.median(), median, "trend-only ignores the season");
    }

    #[test]
    fn short_context_is_flagged_and_flat_at_the_mean() {
        let f = run(&[1.0, 2.0, 3.0, 4.0, 5.0], 3);
        assert!(f.insufficient_history);
        assert_eq!(f.context_points, 5);
        for m in f.median() {
            assert!(close(m, 3.0, 1e-9));
        }
        let single = run(&[7.0], 2);
        assert!(single.insufficient_history);
        assert!(single.degenerate_band, "one point has no spread");
        let (lo, hi) = single.band(0.1, 0.9).unwrap();
        assert!(close(lo[0], 6.993, 1e-9) && close(hi[0], 7.007, 1e-9));
    }

    #[test]
    fn a_constant_context_gets_a_floored_band_and_says_so() {
        // An HTTP status probe: 200 every time.
        let f = run(&[200.0; 40], 4);
        assert!(f.degenerate_band);
        let median = f.median();
        let (lo, hi) = f.band(0.1, 0.9).unwrap();
        for h in 0..4 {
            assert!(close(median[h], 200.0, 1e-9));
            assert!(close(hi[h] - lo[h], 0.4, 1e-9), "0.1 % of 200 each way");
        }
    }

    #[test]
    fn an_exactly_periodic_context_gets_a_floored_band() {
        let context: Vec<f64> = (0..60).map(|i| 300.0 + 5.0 * lead(i % 3)).collect();
        let f = run(&context, 6);
        assert!(f.degenerate_band, "an exact season leaves no residuals");
        let (lo, hi) = f.band(0.1, 0.9).unwrap();
        for h in 0..6 {
            let half = (hi[h] - lo[h]) / 2.0;
            assert!(half >= 2.5 - 1e-9, "half the 5-unit resolution: {half}");
            assert!(half < 10.0, "a floor, not a guess: {half}");
        }
    }

    #[test]
    fn a_noisy_context_keeps_its_own_band() {
        // Deterministic pseudo-noise (an LCG): spread, but no season.
        let mut state: u64 = 7;
        let context: Vec<f64> = (0..50)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                100.0 + f64::from(u8::try_from((state >> 56) & 0x0f).unwrap_or(0)) * 0.5
            })
            .collect();
        assert!(!run(&context, 8).degenerate_band);
    }

    #[test]
    fn gaps_are_interpolated_and_not_counted() {
        let filled = fill_gaps(&[f64::NAN, 1.0, f64::NAN, f64::NAN, 4.0, f64::NAN]);
        assert_eq!(filled, vec![1.0, 1.0, 2.0, 3.0, 4.0, 4.0]);
        let mut context: Vec<f64> = (0..30).map(|i| 5.0 + 0.1 * lead(i)).collect();
        context[3] = f64::NAN;
        context[17] = f64::NAN;
        let f = run(&context, 2);
        assert_eq!(f.context_points, 28);
        assert!(f.median().iter().all(|m| m.is_finite()));
    }

    #[test]
    fn same_input_same_output() {
        let context: Vec<f64> = (0..50)
            .map(|i| (lead(i) * 0.7).sin() * 3.0 + lead(i))
            .collect();
        assert_eq!(run(&context, 8), run(&context, 8));
    }

    #[test]
    fn custom_levels_and_request_errors() {
        let context: Vec<f64> = (0..20).map(lead).collect();
        let levels = [0.05, 0.5, 0.95];
        let f = HoltWinters::new()
            .forecast(&ForecastRequest {
                levels: &levels,
                ..ForecastRequest::new(&context, 2)
            })
            .unwrap();
        assert_eq!(f.levels, levels);
        assert_eq!(f.quantiles[0].len(), 3);
        assert_eq!(
            HoltWinters::new().forecast(&ForecastRequest::new(&context, 0)),
            Err(ForecastError::ZeroHorizon)
        );
    }

    #[test]
    fn no_season_in_noise_free_trend_or_constant() {
        let trend: Vec<f64> = (0..60).map(lead).collect();
        assert_eq!(detect_period(&trend), None);
        assert_eq!(detect_period(&[2.0; 60]), None);
        assert_eq!(detect_period(&[1.0, 2.0, 3.0]), None);
    }
}
