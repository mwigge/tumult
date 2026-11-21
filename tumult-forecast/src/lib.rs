//! Probabilistic forecasting for resilience telemetry.
//!
//! Every chart in Tumult that shows a band gets it from this crate. The
//! [`ForecastProvider`] trait is the seam: the built-in [`HoltWinters`]
//! provider always works (pure Rust, no weights, no network), and an
//! external model such as the Timestral sidecar is an upgrade behind the
//! same trait, never a dependency. Every [`Forecast`] names the model that
//! produced it in its [`Provenance`], so a band on a compliance report can
//! be reproduced.
//!
//! The derived computations are pure functions over a forecast and the
//! observed series:
//!
//! * [`impact::counterfactual`] — what a metric would have done without the
//!   fault, and how far the observed run strayed from it, per sample and
//!   summarised.
//! * [`recovery::recovery_time`] — when the observed series re-entered the
//!   forecast band after the fault ended and stayed there: measured RTO for
//!   disaster-recovery runs, MTTR for chaos runs.
//!
//! [`resample`] turns the lake's irregular `(ts_ns, value)` points into the
//! regular-step series with `NaN` gaps every provider expects (the series
//! contract), capped at [`resample::MAX_POINTS`].
//!
//! A band never collapses to a line: [`band::floor`] is the narrowest
//! half-width a series may be drawn with, and a forecast that had to be
//! widened to it says so in [`Forecast::degenerate_band`].
//!
//! Nothing here touches a database or the network. Everything is
//! deterministic: the Holt-Winters fit is a grid search, so two calls on
//! the same input produce the same band bit for bit.

pub mod band;
pub mod holt_winters;
pub mod impact;
pub mod recovery;
pub mod resample;
pub mod stats;

use std::fmt;

use serde::{Deserialize, Serialize};

pub use holt_winters::HoltWinters;
pub use resample::RegularSeries;

/// The default quantile levels every provider must be able to produce: the
/// 10–90 % band around the median.
pub const DEFAULT_LEVELS: [f64; 3] = [0.1, 0.5, 0.9];

/// Fewer non-gap context points than this and a forecast is flagged
/// [`Forecast::insufficient_history`]: the band is still drawn, but it is a
/// guess, and callers must say so.
pub const MIN_HISTORY: usize = 8;

/// Which model produced a forecast. Stored next to every cached forecast
/// and shown on every chart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    /// Model family, e.g. `holt-winters` or `timestral`.
    pub model: String,
    /// Model or crate version.
    pub version: String,
    /// Hash of the weights for learned models; `none` for closed-form ones.
    pub weights_hash: String,
}

/// Past-only and known-future covariates (the series contract's second
/// dimension). The built-in provider ignores them; learned models use the
/// known-future ones to condition the forecast — the counterfactual sets a
/// fault indicator to zero across the horizon.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Covariates {
    /// One row per covariate, each as long as the context.
    pub past: Vec<Vec<f64>>,
    /// One row per covariate, each as long as the context plus the horizon.
    pub known_future: Vec<Vec<f64>>,
}

/// One forecast request.
#[derive(Debug, Clone, Copy)]
pub struct ForecastRequest<'a> {
    /// Regular-step history, `NaN` for gaps.
    pub context: &'a [f64],
    /// Steps to forecast.
    pub horizon: usize,
    /// Quantile levels wanted, each in `(0, 1)`; must include `0.5`.
    pub levels: &'a [f64],
    pub covariates: Option<&'a Covariates>,
}

impl<'a> ForecastRequest<'a> {
    /// A request at the [`DEFAULT_LEVELS`] with no covariates.
    #[must_use]
    pub fn new(context: &'a [f64], horizon: usize) -> Self {
        Self {
            context,
            horizon,
            levels: &DEFAULT_LEVELS,
            covariates: None,
        }
    }
}

/// A probabilistic forecast: `quantiles[step][level]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Forecast {
    pub levels: Vec<f64>,
    pub quantiles: Vec<Vec<f64>>,
    /// Non-gap context points the model saw.
    pub context_points: usize,
    /// Fewer than [`MIN_HISTORY`] context points: the band is a guess.
    pub insufficient_history: bool,
    /// The model's own band was narrower than the series' [`band::floor`]
    /// at one step or more and was widened to it: the width shown is a
    /// floor, not a measurement.
    #[serde(default)]
    pub degenerate_band: bool,
    pub provenance: Provenance,
}

impl Forecast {
    /// Steps in the horizon.
    #[must_use]
    pub fn horizon(&self) -> usize {
        self.quantiles.len()
    }

    /// The series at one quantile level, when the forecast carries it.
    #[must_use]
    pub fn quantile(&self, level: f64) -> Option<Vec<f64>> {
        let idx = self.levels.iter().position(|l| (l - level).abs() < 1e-9)?;
        Some(self.quantiles.iter().map(|row| row[idx]).collect())
    }

    /// The median series.
    #[must_use]
    pub fn median(&self) -> Vec<f64> {
        self.quantile(0.5).unwrap_or_default()
    }

    /// The `(lower, upper)` band between two levels, when both exist.
    #[must_use]
    pub fn band(&self, lower: f64, upper: f64) -> Option<(Vec<f64>, Vec<f64>)> {
        Some((self.quantile(lower)?, self.quantile(upper)?))
    }

    /// Widen every step whose outermost levels are less than `2 * floor`
    /// apart so that they are exactly that far apart, the other levels
    /// spread in proportion to their distance from the median. Steps with
    /// a gap at an outer level are left alone. Returns whether any step
    /// was widened, and sets [`Forecast::degenerate_band`] when one was.
    pub fn apply_band_floor(&mut self, floor: f64) -> bool {
        let Some(spread) = LevelSpread::of(&self.levels) else {
            return false;
        };
        let mut widened = false;
        for row in &mut self.quantiles {
            widened |= spread.widen(row, floor);
        }
        self.degenerate_band |= widened;
        widened
    }
}

/// Where the levels of a forecast sit relative to its centre.
struct LevelSpread {
    lowest: usize,
    highest: usize,
    median: Option<usize>,
    /// `(level - centre level) / half the level range`, per level.
    offsets: Vec<f64>,
}

impl LevelSpread {
    /// `None` when the levels span no range (one level, or none).
    fn of(levels: &[f64]) -> Option<Self> {
        let lowest = index_of(levels, |a, b| a < b)?;
        let highest = index_of(levels, |a, b| a > b)?;
        let half_range = (levels[highest] - levels[lowest]) / 2.0;
        if !(half_range.is_finite() && half_range > 0.0) {
            return None;
        }
        let median = levels.iter().position(|l| (l - 0.5).abs() < 1e-9);
        let centre = match median {
            Some(_) => 0.5,
            None => f64::midpoint(levels[lowest], levels[highest]),
        };
        Some(Self {
            lowest,
            highest,
            median,
            offsets: levels.iter().map(|l| (l - centre) / half_range).collect(),
        })
    }

    /// Rewrite `row` around its centre when it is narrower than the floor.
    fn widen(&self, row: &mut [f64], floor: f64) -> bool {
        let (low, high) = (row[self.lowest], row[self.highest]);
        let half_width = (high - low) / 2.0;
        if !half_width.is_finite() || half_width >= floor {
            return false;
        }
        let centre = match self.median {
            Some(i) if row[i].is_finite() => row[i],
            _ => f64::midpoint(low, high),
        };
        for (value, offset) in row.iter_mut().zip(&self.offsets) {
            *value = centre + floor * offset;
        }
        true
    }
}

/// Index of the level that wins `better` against every other one.
fn index_of(levels: &[f64], better: impl Fn(f64, f64) -> bool) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (i, level) in levels.iter().enumerate() {
        if best.is_none_or(|b| better(*level, levels[b])) {
            best = Some(i);
        }
    }
    best
}

/// Why a provider could not forecast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForecastError {
    /// No finite point in the context at all (gaps and infinities only).
    EmptyContext,
    /// `horizon == 0`.
    ZeroHorizon,
    /// Levels outside `(0, 1)`, or no `0.5`.
    InvalidLevels(String),
    /// A derived computation's window does not fit the series (no
    /// pre-fault context, fault window past the end, and so on).
    InvalidWindow(String),
    /// A remote model failed; the message is for the log, not the client.
    Provider(String),
}

impl fmt::Display for ForecastError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyContext => write!(f, "forecast context has no observed points"),
            Self::ZeroHorizon => write!(f, "forecast horizon must be at least one step"),
            Self::InvalidLevels(msg) => write!(f, "invalid quantile levels: {msg}"),
            Self::InvalidWindow(msg) => write!(f, "invalid window: {msg}"),
            Self::Provider(msg) => write!(f, "forecast provider failed: {msg}"),
        }
    }
}

impl std::error::Error for ForecastError {}

/// A model that turns a regular-step context into quantile paths.
pub trait ForecastProvider {
    /// Forecast `request.horizon` steps past the end of the context.
    ///
    /// # Errors
    /// [`ForecastError::EmptyContext`] when nothing is observed,
    /// [`ForecastError::ZeroHorizon`] for an empty horizon,
    /// [`ForecastError::InvalidLevels`] for levels outside `(0, 1)` or
    /// without the median, [`ForecastError::Provider`] when the model fails.
    fn forecast(&self, request: &ForecastRequest<'_>) -> Result<Forecast, ForecastError>;

    /// The model family this provider reports in [`Provenance::model`].
    fn model(&self) -> &'static str;
}

/// Validate the shared request invariants every provider relies on.
///
/// # Errors
/// See [`ForecastProvider::forecast`].
pub fn validate(request: &ForecastRequest<'_>) -> Result<(), ForecastError> {
    if request.horizon == 0 {
        return Err(ForecastError::ZeroHorizon);
    }
    if request.levels.is_empty() {
        return Err(ForecastError::InvalidLevels("no levels requested".into()));
    }
    if let Some(bad) = request
        .levels
        .iter()
        .find(|l| !(l.is_finite() && **l > 0.0 && **l < 1.0))
    {
        return Err(ForecastError::InvalidLevels(format!(
            "level {bad} is outside (0, 1)"
        )));
    }
    if !request.levels.iter().any(|l| (l - 0.5).abs() < 1e-9) {
        return Err(ForecastError::InvalidLevels(
            "the median (0.5) must be requested".into(),
        ));
    }
    if !request.context.iter().any(|v| v.is_finite()) {
        return Err(ForecastError::EmptyContext);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn forecast() -> Forecast {
        Forecast {
            levels: vec![0.1, 0.5, 0.9],
            quantiles: vec![vec![1.0, 2.0, 3.0], vec![1.5, 2.5, 3.5]],
            context_points: 10,
            insufficient_history: false,
            degenerate_band: false,
            provenance: Provenance {
                model: "test".into(),
                version: "0".into(),
                weights_hash: "none".into(),
            },
        }
    }

    #[test]
    fn quantile_accessors_pick_columns() {
        let f = forecast();
        assert_eq!(f.horizon(), 2);
        assert_eq!(f.median(), vec![2.0, 2.5]);
        let (lo, hi) = f.band(0.1, 0.9).unwrap();
        assert_eq!(lo, vec![1.0, 1.5]);
        assert_eq!(hi, vec![3.0, 3.5]);
        assert!(f.quantile(0.25).is_none());
        assert!(f.band(0.1, 0.25).is_none());
    }

    #[test]
    fn validate_rejects_bad_requests() {
        let ctx = [1.0, 2.0, 3.0];
        assert_eq!(
            validate(&ForecastRequest {
                horizon: 0,
                ..ForecastRequest::new(&ctx, 1)
            }),
            Err(ForecastError::ZeroHorizon)
        );
        let all_gaps = [f64::NAN, f64::NAN];
        assert_eq!(
            validate(&ForecastRequest::new(&all_gaps, 1)),
            Err(ForecastError::EmptyContext)
        );
        let no_median = [0.1, 0.9];
        assert!(matches!(
            validate(&ForecastRequest {
                levels: &no_median,
                ..ForecastRequest::new(&ctx, 1)
            }),
            Err(ForecastError::InvalidLevels(_))
        ));
        let out_of_range = [0.5, 1.0];
        assert!(matches!(
            validate(&ForecastRequest {
                levels: &out_of_range,
                ..ForecastRequest::new(&ctx, 1)
            }),
            Err(ForecastError::InvalidLevels(_))
        ));
        assert_eq!(validate(&ForecastRequest::new(&ctx, 1)), Ok(()));
    }

    #[test]
    fn a_context_of_infinities_and_gaps_is_empty() {
        let nothing_observed = [f64::INFINITY, f64::NAN, f64::NEG_INFINITY];
        assert_eq!(
            validate(&ForecastRequest::new(&nothing_observed, 1)),
            Err(ForecastError::EmptyContext)
        );
    }

    #[test]
    fn a_band_narrower_than_the_floor_is_widened_and_flagged() {
        let mut f = forecast();
        assert!(!f.apply_band_floor(0.5), "both steps are one wide already");
        assert!(!f.degenerate_band);
        assert_eq!(f.quantiles[0], vec![1.0, 2.0, 3.0]);

        f.quantiles[1] = vec![2.5, 2.5, 2.5];
        assert!(f.apply_band_floor(0.5));
        assert!(f.degenerate_band);
        assert_eq!(f.quantiles[0], vec![1.0, 2.0, 3.0], "wide steps are kept");
        assert_eq!(f.quantiles[1], vec![2.0, 2.5, 3.0]);
    }

    #[test]
    fn the_floor_scales_with_the_level_spacing_and_skips_gaps() {
        let mut f = forecast();
        f.levels = vec![0.25, 0.5, 0.9];
        f.quantiles = vec![vec![4.0, 4.0, 4.0], vec![f64::NAN, 4.0, f64::NAN]];
        assert!(f.apply_band_floor(1.3));
        // The outer levels end up 2 * 1.3 apart, each on its own side.
        let row = &f.quantiles[0];
        assert!((row[2] - row[0] - 2.6).abs() < 1e-12, "{row:?}");
        assert!((row[1] - 4.0).abs() < 1e-12 && row[0] < 4.0 && row[2] > 4.0);
        assert!(f.quantiles[1][0].is_nan(), "a gap stays a gap");

        let mut median_only = forecast();
        median_only.levels = vec![0.5];
        median_only.quantiles = vec![vec![1.0]];
        assert!(!median_only.apply_band_floor(1.0), "one level has no width");
    }

    #[test]
    fn errors_display_for_logs() {
        assert!(ForecastError::Provider("boom".into())
            .to_string()
            .contains("boom"));
        assert!(ForecastError::EmptyContext
            .to_string()
            .contains("no observed"));
    }
}
