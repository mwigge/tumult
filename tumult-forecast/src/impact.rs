//! Counterfactual impact of a run: forecast the metric as if no fault had
//! happened, then measure how far the observed run strayed from that
//! forecast — per sample, normalised by the band, and summarised — and
//! when it came back.
//!
//! The fault indicator goes to the provider as a known-future covariate
//! set to zero across the horizon (the series contract's second
//! dimension): a covariate-aware model reads that as "no fault"; the
//! built-in provider ignores it, which is still a fair baseline.
//!
//! Two references exist. [`Reference::Baseline`] forecasts forward from the
//! samples before the fault — the principled one. [`Reference::PostBackcast`]
//! is for runs that recorded no pre-fault samples (the runner samples the
//! during and post phases): the post-phase samples, reversed, are the
//! context, the forecast runs back across the fault window and is flipped
//! into time order, and the post phase is judged against its own empirical
//! band. [`plan`] picks the reference from the fault window; every
//! [`Impact`] says which one it used.
//!
//! The computation is split in two so a caller can obtain the forecast
//! asynchronously (a remote model) between the steps: [`plan`] builds the
//! [`Plan`] with its [`Plan::request`], and [`Plan::evaluate`] turns the
//! provider's answer into the [`Impact`]. [`counterfactual`] does both with
//! a synchronous provider.

use serde::Serialize;

use crate::recovery::{recovery_time, Recovery};
use crate::{
    Covariates, Forecast, ForecastError, ForecastProvider, ForecastRequest, DEFAULT_LEVELS,
};

/// Which samples the counterfactual was built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reference {
    /// Forecast forward from the samples before the fault.
    Baseline,
    /// Backcast from the post-phase samples (no pre-fault samples).
    PostBackcast,
}

/// Where the fault was, as step indices into the series: `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FaultWindow {
    pub start: usize,
    pub end: usize,
}

/// One step compared with its counterfactual. `impact` is
/// `(observed − median) / half band width`: `+1` sits on the upper edge of
/// the 10–90 band, `0` on the median; `NaN` when the step is a gap or the
/// band has no width.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImpactSample {
    /// Index into the full series.
    pub index: usize,
    pub observed: f64,
    pub median: f64,
    pub lower: f64,
    pub upper: f64,
    pub impact: f64,
}

/// The run's impact in a few numbers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImpactSummary {
    /// Mean normalised impact across the fault window (gaps excluded).
    pub during_mean: f64,
    /// Largest absolute normalised impact in the fault window.
    pub during_peak: f64,
    /// Index into the full series of that peak.
    pub during_peak_index: Option<usize>,
    /// Share of observed fault-window steps outside the band.
    pub during_outside_ratio: f64,
    /// Mean normalised impact after the fault window.
    pub post_mean: f64,
    pub samples_during: usize,
    pub samples_post: usize,
}

/// The counterfactual for one series.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Impact {
    pub reference: Reference,
    pub fault: FaultWindow,
    /// The counterfactual quantiles in time order: over `fault.start..len`
    /// for [`Reference::Baseline`], over `0..fault.end` for
    /// [`Reference::PostBackcast`].
    pub forecast: Forecast,
    /// One entry per compared step (`fault.start..len`), in time order.
    pub samples: Vec<ImpactSample>,
    pub summary: ImpactSummary,
    pub recovery: Recovery,
}

/// A counterfactual ready for its forecast.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub reference: Reference,
    pub fault: FaultWindow,
    /// The provider's context: the pre-fault samples, or the post-phase
    /// samples reversed.
    pub context: Vec<f64>,
    pub horizon: usize,
    /// The zeroed fault indicator.
    pub covariates: Covariates,
}

impl Plan {
    /// The request to hand a provider.
    #[must_use]
    pub fn request(&self) -> ForecastRequest<'_> {
        ForecastRequest {
            context: &self.context,
            horizon: self.horizon,
            levels: &DEFAULT_LEVELS,
            covariates: Some(&self.covariates),
        }
    }

    /// Turn the provider's answer into the impact. `values` must be the
    /// series the plan was built from.
    ///
    /// # Errors
    /// [`ForecastError::Provider`] when the forecast lacks the 10–90 band
    /// or does not cover the horizon.
    pub fn evaluate(
        &self,
        mut forecast: Forecast,
        values: &[f64],
        stable_steps: usize,
    ) -> Result<Impact, ForecastError> {
        if forecast.horizon() != self.horizon {
            return Err(ForecastError::Provider(format!(
                "forecast covers {} steps, the plan needs {}",
                forecast.horizon(),
                self.horizon
            )));
        }
        if self.reference == Reference::PostBackcast {
            forecast.quantiles.reverse();
        }
        let median = forecast.median();
        let (lower, upper) = forecast
            .band(0.1, 0.9)
            .ok_or_else(|| ForecastError::Provider("forecast lacks the 10-90 band".into()))?;
        let fault = self.fault;
        let band_at = |i: usize| -> (f64, f64, f64) {
            match self.reference {
                Reference::Baseline => {
                    let h = i - fault.start;
                    (lower[h], median[h], upper[h])
                }
                Reference::PostBackcast if i < fault.end => (lower[i], median[i], upper[i]),
                Reference::PostBackcast => {
                    let post = &values[fault.end..];
                    (
                        crate::stats::quantile(post, 0.1),
                        crate::stats::quantile(post, 0.5),
                        crate::stats::quantile(post, 0.9),
                    )
                }
            }
        };
        let samples: Vec<ImpactSample> = (fault.start..values.len())
            .map(|i| {
                let (lo, med, hi) = band_at(i);
                ImpactSample {
                    index: i,
                    observed: values[i],
                    median: med,
                    lower: lo,
                    upper: hi,
                    impact: normalised(values[i], med, lo, hi),
                }
            })
            .collect();
        let during = fault.end - fault.start;
        let summary = summarise(&samples, during);
        let observed: Vec<f64> = samples.iter().map(|s| s.observed).collect();
        let lower_all: Vec<f64> = samples.iter().map(|s| s.lower).collect();
        let upper_all: Vec<f64> = samples.iter().map(|s| s.upper).collect();
        let recovery = recovery_time(&observed, &lower_all, &upper_all, during, stable_steps);
        Ok(Impact {
            reference: self.reference,
            fault,
            forecast,
            samples,
            summary,
            recovery,
        })
    }
}

/// Build the plan: a forward forecast when there are samples before the
/// fault, a post-phase backcast otherwise. The fault end is clamped to the
/// series.
///
/// # Errors
/// [`ForecastError::InvalidWindow`] when the fault starts at or after the
/// last sample, holds no samples, or (for a backcast) no samples follow it.
pub fn plan(values: &[f64], fault: FaultWindow) -> Result<Plan, ForecastError> {
    if fault.start >= values.len() {
        return Err(ForecastError::InvalidWindow(
            "the fault starts at or after the last sample".into(),
        ));
    }
    let fault = FaultWindow {
        start: fault.start,
        end: fault.end.clamp(fault.start, values.len()),
    };
    if fault.start > 0 {
        let context = values[..fault.start].to_vec();
        let horizon = values.len() - fault.start;
        return Ok(Plan {
            reference: Reference::Baseline,
            fault,
            covariates: indicator(context.len(), values.len()),
            context,
            horizon,
        });
    }
    if fault.end == fault.start {
        return Err(ForecastError::InvalidWindow(
            "the fault window holds no samples".into(),
        ));
    }
    if fault.end >= values.len() {
        return Err(ForecastError::InvalidWindow(
            "no samples before or after the fault to forecast from".into(),
        ));
    }
    let context: Vec<f64> = values[fault.end..].iter().rev().copied().collect();
    Ok(Plan {
        reference: Reference::PostBackcast,
        fault,
        covariates: indicator(context.len(), values.len()),
        context,
        horizon: fault.end,
    })
}

fn indicator(context_len: usize, total_len: usize) -> Covariates {
    Covariates {
        past: vec![vec![0.0; context_len]],
        known_future: vec![vec![0.0; total_len]],
    }
}

/// [`plan`] + the provider + [`Plan::evaluate`] in one call.
///
/// # Errors
/// See [`plan`] and [`Plan::evaluate`]; provider errors pass through.
pub fn counterfactual(
    provider: &dyn ForecastProvider,
    values: &[f64],
    fault: FaultWindow,
    stable_steps: usize,
) -> Result<Impact, ForecastError> {
    let plan = plan(values, fault)?;
    let forecast = provider.forecast(&plan.request())?;
    plan.evaluate(forecast, values, stable_steps)
}

fn normalised(observed: f64, median: f64, lower: f64, upper: f64) -> f64 {
    let half_width = (upper - lower) / 2.0;
    if !observed.is_finite() || !half_width.is_finite() || half_width <= 0.0 {
        return f64::NAN;
    }
    (observed - median) / half_width
}

#[allow(clippy::cast_precision_loss)] // sample counts are far below 2^52
fn summarise(samples: &[ImpactSample], during: usize) -> ImpactSummary {
    let (fault, post) = samples.split_at(during.min(samples.len()));
    let impacts = |xs: &[ImpactSample]| -> Vec<f64> {
        xs.iter()
            .map(|s| s.impact)
            .filter(|i| i.is_finite())
            .collect()
    };
    let during_impacts = impacts(fault);
    let peak = fault
        .iter()
        .filter(|s| s.impact.is_finite())
        .max_by(|a, b| a.impact.abs().total_cmp(&b.impact.abs()));
    let observed_during = fault.iter().filter(|s| s.observed.is_finite()).count();
    let outside_during = fault
        .iter()
        .filter(|s| s.observed.is_finite() && (s.observed < s.lower || s.observed > s.upper))
        .count();
    ImpactSummary {
        during_mean: crate::stats::mean(&during_impacts),
        during_peak: peak.map_or(f64::NAN, |s| s.impact),
        during_peak_index: peak.map(|s| s.index),
        during_outside_ratio: if observed_during == 0 {
            f64::NAN
        } else {
            outside_during as f64 / observed_during as f64
        },
        post_mean: crate::stats::mean(&impacts(post)),
        samples_during: fault.len(),
        samples_post: post.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HoltWinters, Provenance};

    /// A provider that always answers "flat at 10, band 8–12".
    struct Flat;

    impl ForecastProvider for Flat {
        fn forecast(&self, request: &ForecastRequest<'_>) -> Result<Forecast, ForecastError> {
            crate::validate(request)?;
            assert!(
                request.covariates.is_some(),
                "the fault indicator is passed"
            );
            Ok(Forecast {
                levels: request.levels.to_vec(),
                quantiles: (0..request.horizon)
                    .map(|_| {
                        request
                            .levels
                            .iter()
                            .map(|l| 10.0 + (l - 0.5) * 5.0)
                            .collect()
                    })
                    .collect(),
                context_points: request.context.len(),
                insufficient_history: false,
                provenance: Provenance {
                    model: "flat".into(),
                    version: "0".into(),
                    weights_hash: "none".into(),
                },
            })
        }

        fn model(&self) -> &'static str {
            "flat"
        }
    }

    #[test]
    fn impact_is_normalised_by_the_band_and_summarised() {
        // 4 pre-fault, fault over 4..7 (3 steps), 3 post steps.
        let values = [
            10.0,
            10.0,
            10.0,
            10.0,
            14.0,
            16.0,
            f64::NAN,
            11.0,
            10.0,
            10.0,
        ];
        let impact = counterfactual(&Flat, &values, FaultWindow { start: 4, end: 7 }, 2).unwrap();
        assert_eq!(impact.forecast.horizon(), 6);
        assert_eq!(impact.samples.len(), 6);
        assert_eq!(impact.samples[0].index, 4);
        assert!(
            (impact.samples[0].impact - 2.0).abs() < 1e-9,
            "14 is two half-widths up"
        );
        assert!((impact.samples[1].impact - 3.0).abs() < 1e-9);
        assert!(impact.samples[2].impact.is_nan(), "gap");
        let s = &impact.summary;
        assert!((s.during_mean - 2.5).abs() < 1e-9);
        assert!((s.during_peak - 3.0).abs() < 1e-9);
        assert_eq!(s.during_peak_index, Some(5));
        assert!((s.during_outside_ratio - 1.0).abs() < 1e-9);
        assert_eq!(s.samples_during, 3);
        assert_eq!(s.samples_post, 3);
        assert!((s.post_mean - (0.5 / 3.0)).abs() < 1e-9);
        assert_eq!(impact.recovery.recovered_at, Some(3));
        assert_eq!(impact.recovery.steps_after_fault, Some(0));
        assert_eq!(impact.fault, FaultWindow { start: 4, end: 7 });
        assert_eq!(impact.forecast.provenance.model, "flat");
        assert_eq!(impact.reference, Reference::Baseline);
    }

    #[test]
    fn plan_picks_the_reference_and_checks_the_window() {
        let values = [1.0, 2.0, 3.0, 4.0];
        let forward = plan(&values, FaultWindow { start: 2, end: 99 }).unwrap();
        assert_eq!(forward.reference, Reference::Baseline);
        assert_eq!(forward.fault.end, 4, "the end is clamped");
        assert_eq!(forward.context, vec![1.0, 2.0]);
        assert_eq!(forward.horizon, 2);
        assert_eq!(forward.covariates.known_future[0].len(), 4);
        assert_eq!(forward.request().horizon, 2);

        let back = plan(&values, FaultWindow { start: 0, end: 2 }).unwrap();
        assert_eq!(back.reference, Reference::PostBackcast);
        assert_eq!(back.context, vec![4.0, 3.0], "post phase, reversed");
        assert_eq!(back.horizon, 2);

        for fault in [
            FaultWindow { start: 4, end: 4 },
            FaultWindow { start: 0, end: 0 },
            FaultWindow { start: 0, end: 4 },
        ] {
            assert!(
                matches!(plan(&values, fault), Err(ForecastError::InvalidWindow(_))),
                "{fault:?}"
            );
        }
    }

    #[test]
    fn evaluate_rejects_a_forecast_of_the_wrong_length() {
        let values = [1.0, 2.0, 3.0, 4.0];
        let p = plan(&values, FaultWindow { start: 2, end: 4 }).unwrap();
        let mut short = Flat.forecast(&p.request()).unwrap();
        short.quantiles.truncate(1);
        assert!(matches!(
            p.evaluate(short, &values, 1),
            Err(ForecastError::Provider(_))
        ));
    }

    #[test]
    fn backcast_uses_the_post_phase_as_the_reference() {
        // No pre-fault samples: fault over 0..3, then five post samples
        // settling around 10 (their own 10–90 band is about 9.4–11.3).
        let values = [14.0, 16.0, 13.0, 11.5, 10.0, 9.0, 10.0, 11.0];
        let impact = counterfactual(&Flat, &values, FaultWindow { start: 0, end: 3 }, 2).unwrap();
        assert_eq!(impact.reference, Reference::PostBackcast);
        assert_eq!(
            impact.forecast.horizon(),
            3,
            "backcast covers the fault window"
        );
        assert_eq!(impact.samples.len(), 8);
        assert!(
            (impact.samples[0].impact - 2.0).abs() < 1e-9,
            "flat provider: 14 vs 10±2"
        );
        assert_eq!(impact.summary.samples_during, 3);
        assert_eq!(impact.summary.samples_post, 5);
        let post = &impact.samples[3];
        assert!(post.lower < 10.0 && post.upper > 10.0);
        assert!(post.observed > post.upper, "11.5 is above the post band");
        assert!(
            impact.samples[5].observed < impact.samples[5].lower,
            "9 is below it"
        );
        assert_eq!(impact.recovery.recovered_at, Some(6));
        assert_eq!(impact.recovery.steps_after_fault, Some(3));
        assert_eq!(impact.recovery.outside_during_fault, 3);
    }

    #[test]
    fn zero_width_band_yields_nan_impact() {
        assert!(normalised(1.0, 1.0, 1.0, 1.0).is_nan());
        assert!(normalised(f64::NAN, 1.0, 0.0, 2.0).is_nan());
        assert!((normalised(1.5, 1.0, 0.0, 2.0) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn works_end_to_end_with_the_built_in_provider() {
        // Deterministic pseudo-noise (an LCG), so the band has width but no
        // exact season the fit could absorb.
        let mut state: u64 = 42;
        let mut values: Vec<f64> = (0..40)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                100.0 + f64::from(u8::try_from((state >> 56) & 0x0f).unwrap_or(0)) * 0.5
            })
            .collect();
        values.extend([160.0, 170.0, 165.0, 120.0, 101.0, 100.0, 101.0, 100.0]);
        let impact = counterfactual(
            &HoltWinters::new(),
            &values,
            FaultWindow { start: 40, end: 43 },
            2,
        )
        .unwrap();
        assert!(impact.summary.during_peak > 1.0, "{:?}", impact.summary);
        assert!(impact.summary.during_outside_ratio > 0.99);
        assert!(impact.recovery.recovered_at.is_some());
        assert_eq!(impact.forecast.provenance.model, "holt-winters");
        assert_eq!(impact.samples.len(), 8);
    }
}
