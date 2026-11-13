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
//! band. Post samples still recovering right after the fault are left out
//! of both the context and that band, or the backcast would extend the
//! recovery slope across the fault window and the band would count the
//! recovery as normal. A later post sample far outside the settled range (a
//! stray spike) stays an observation but is a gap in the context, so a
//! single outlier next to the fault cannot bend the backcast.
//!
//! [`plan`] picks the reference from the fault window; every [`Impact`]
//! says which one it used.
//!
//! # Limits worth knowing
//!
//! * **The post band is a self-quantile.** In a backcast the post phase is
//!   judged against the 10-90 band of its own samples, so by construction
//!   about a fifth of perfectly settled post samples fall outside it. The
//!   recovery rule absorbs that (it wants `stable_steps` consecutive
//!   samples inside), but a post-phase "outside" count from a backcast is
//!   biased upwards and must not be read as residual impact. A
//!   [`Reference::Baseline`] counterfactual has no such bias.
//! * **A band has a floor.** A probe that only ever reports `200`, `true`
//!   or a quantised gauge leaves a band of zero width, and an impact
//!   normalised by it would be undefined or, with float noise, absurdly
//!   large. Every band is therefore at least [`crate::band::floor`] of the
//!   reference samples wide on each side — the pre-fault samples for a
//!   baseline, the settled post samples for a backcast, never the fault's
//!   own — and an [`Impact`] whose band had to be widened says so in
//!   [`Impact::degenerate_band`]: its impact numbers are in units of that
//!   floor, not of an observed spread, and can be very large.
//! * **Strays are gapped sparingly.** A post sample is left out of the
//!   backcast's context when it lies more than [`FENCE_WIDTHS`] widths
//!   outside the middle half of the kept samples (the width is their
//!   interquartile range, at least one step of a quantised metric). When
//!   that would leave out more than a tenth of the samples (and more than
//!   one), none is left out: values that keep coming back are a level of
//!   the metric, not strays.
//! * **The zeroed fault indicator carries no information on its own.** A
//!   covariate that is zero over both the context and the horizon tells a
//!   model nothing it can learn from; it only matters to a model that has
//!   seen the indicator non-zero. It is sent so the request has the same
//!   shape as one that does carry a fault history.
//!
//! The computation is split in two so a caller can obtain the forecast
//! asynchronously (a remote model) between the steps: [`plan`] builds the
//! [`Plan`] with its [`Plan::request`], and [`Plan::evaluate`] turns the
//! provider's answer into the [`Impact`]. [`counterfactual`] does both with
//! a synchronous provider.

use serde::Serialize;

use crate::recovery::{recovery_time, Recovery};
use crate::{
    band, stats, Covariates, Forecast, ForecastError, ForecastProvider, ForecastRequest,
    DEFAULT_LEVELS,
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
/// the 10–90 band, `0` on the median; `NaN` when the step is a gap.
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
    /// A band the samples were judged against (the forecast's, or the post
    /// phase's own) was narrower than the reference's [`band::floor`] and
    /// was widened to it: the impact is in units of that floor.
    pub degenerate_band: bool,
}

/// A counterfactual ready for its forecast.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub reference: Reference,
    pub fault: FaultWindow,
    /// The provider's context: the pre-fault samples, or the settled
    /// post-phase samples reversed.
    pub context: Vec<f64>,
    /// Steps to forecast. A backcast also spans the post-phase samples
    /// left out of the context as still recovering.
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
            // Backcast steps run away from the context: first the skipped
            // recovery tail, then the fault window. Keep the window.
            forecast.quantiles.reverse();
            forecast.quantiles.truncate(self.fault.end);
        }
        let floor = band::floor(self.reference_samples(values));
        forecast.apply_band_floor(floor);
        let median = forecast.median();
        let (lower, upper) = forecast
            .band(0.1, 0.9)
            .ok_or_else(|| ForecastError::Provider("forecast lacks the 10-90 band".into()))?;
        let fault = self.fault;
        let post = self.post_band(values, floor);
        let band_at = |i: usize| -> (f64, f64, f64) {
            match &post {
                None => {
                    let h = i - fault.start;
                    (lower[h], median[h], upper[h])
                }
                Some(_) if i < fault.end => (lower[i], median[i], upper[i]),
                Some(own) => (own.lower, own.median, own.upper),
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
            degenerate_band: forecast.degenerate_band || post.is_some_and(|own| own.floored),
            forecast,
            samples,
            summary,
            recovery,
        })
    }

    /// The samples the band stands for, and so the only ones its floor may
    /// be scaled by: the pre-fault samples for a baseline, the settled
    /// post samples for a backcast. Never the fault window's.
    fn reference_samples<'a>(&'a self, values: &'a [f64]) -> &'a [f64] {
        match self.reference {
            Reference::Baseline => &self.context,
            Reference::PostBackcast => &values[self.horizon.min(values.len())..],
        }
    }

    /// The settled post phase's own 10-90 band, computed once: what a
    /// backcast judges the post samples against. `None` for a baseline.
    fn post_band(&self, values: &[f64], floor: f64) -> Option<OwnBand> {
        if self.reference != Reference::PostBackcast {
            return None;
        }
        Some(OwnBand::of(self.reference_samples(values), floor))
    }
}

/// A sample set's own 10-90 band, at least `floor` wide on each side.
#[derive(Debug, Clone, Copy)]
struct OwnBand {
    lower: f64,
    median: f64,
    upper: f64,
    floored: bool,
}

impl OwnBand {
    fn of(samples: &[f64], floor: f64) -> Self {
        let lower = stats::quantile(samples, 0.1);
        let median = stats::quantile(samples, 0.5);
        let upper = stats::quantile(samples, 0.9);
        let half_width = (upper - lower) / 2.0;
        if !half_width.is_finite() || half_width >= floor {
            return Self {
                lower,
                median,
                upper,
                floored: false,
            };
        }
        Self {
            lower: median - floor,
            median,
            upper: median + floor,
            floored: true,
        }
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
    let post = &values[fault.end..];
    let skipped = unsettled_head(post);
    let context = backcast_context(post, skipped);
    Ok(Plan {
        reference: Reference::PostBackcast,
        fault,
        covariates: indicator(context.len(), values.len()),
        context,
        horizon: fault.end + skipped,
    })
}

/// Fewer post samples than this are too few to tell a recovery tail from
/// the settled level.
const MIN_POST_FOR_SKIP: usize = 4;

/// How many leading post-phase samples are still recovering: the run
/// before the first sample inside the floored 10-90 band of the later half
/// of the phase (the samples furthest from the fault) — the same kind of
/// band the post samples are judged against. Never more than the earlier
/// half, and gaps do not end the run.
fn unsettled_head(post: &[f64]) -> usize {
    if post.len() < MIN_POST_FOR_SKIP {
        return 0;
    }
    let half = post.len() / 2;
    let settled = &post[half..];
    let band = OwnBand::of(settled, band::floor(settled));
    post[..half]
        .iter()
        .position(|v| *v >= band.lower && *v <= band.upper)
        .unwrap_or(half)
}

/// How far outside the middle half of the kept post samples one may lie
/// and still be context, in widths (see [`fence`]).
pub const FENCE_WIDTHS: f64 = 3.0;

/// The backcast's context: the post samples after the skipped head,
/// reversed, with strays turned into gaps. One spike next to the fault
/// would otherwise be the last thing the provider sees, and a trend fitted
/// through it runs straight across the fault window. Values that are not
/// rare are not strays: when more than [`allowed_gaps`] samples lie beyond
/// the fence, the context is left as observed.
fn backcast_context(post: &[f64], skipped: usize) -> Vec<f64> {
    let kept: Vec<f64> = post[skipped..].iter().rev().copied().collect();
    let Some((lower, upper)) = fence(&kept) else {
        return kept;
    };
    let stray = |v: f64| v < lower || v > upper;
    let observed = kept.iter().filter(|v| v.is_finite()).count();
    let strays = kept.iter().filter(|v| stray(**v)).count();
    if strays > allowed_gaps(observed) {
        return kept;
    }
    kept.into_iter()
        .map(|v| if stray(v) { f64::NAN } else { v })
        .collect()
}

/// How many of `observed` samples may be gapped as strays: a tenth, and
/// always one, so a short phase can still lose a single spike.
fn allowed_gaps(observed: usize) -> usize {
    (observed / 10).max(1)
}

/// The range a kept post sample is expected in: its interquartile range,
/// [`FENCE_WIDTHS`] widths to each side. The width is the interquartile
/// range itself, at least one step of the metric (the smallest gap between
/// its values, once three distinct values show there is a grid) and at
/// least the band floor, so an integer metric that sits on one value most
/// of the time still has room for its neighbours. `None` when there are
/// too few samples to judge.
fn fence(kept: &[f64]) -> Option<(f64, f64)> {
    if kept.len() < MIN_POST_FOR_SKIP {
        return None;
    }
    let lower = stats::quantile(kept, 0.25);
    let upper = stats::quantile(kept, 0.75);
    let width = (upper - lower)
        .max(grid_step(kept))
        .max(2.0 * band::floor(kept));
    if !width.is_finite() {
        return None;
    }
    Some((lower - FENCE_WIDTHS * width, upper + FENCE_WIDTHS * width))
}

/// One step of a quantised metric, or zero when fewer than three distinct
/// values are present: two values alone may be a level and a stray.
fn grid_step(kept: &[f64]) -> f64 {
    if stats::distinct(kept) < 3 {
        return 0.0;
    }
    stats::resolution(kept).unwrap_or(0.0)
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
                degenerate_band: false,
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
        // No pre-fault samples: fault over 0..3, then eight post samples
        // settling around 10 (the settled ones' 10–90 band is 9.6–10.4).
        let values = [
            14.0, 16.0, 13.0, 11.5, 10.0, 9.0, 10.0, 10.0, 11.0, 10.0, 10.0,
        ];
        let impact = counterfactual(&Flat, &values, FaultWindow { start: 0, end: 3 }, 2).unwrap();
        assert_eq!(impact.reference, Reference::PostBackcast);
        assert_eq!(
            impact.forecast.horizon(),
            3,
            "backcast covers the fault window"
        );
        assert_eq!(impact.samples.len(), 11);
        assert!(
            (impact.samples[0].impact - 2.0).abs() < 1e-9,
            "flat provider: 14 vs 10±2"
        );
        assert_eq!(impact.summary.samples_during, 3);
        assert_eq!(impact.summary.samples_post, 8);
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
    fn backcast_context_skips_the_unsettled_recovery_tail() {
        // Fault over 0..3; the first two post samples are still recovering
        // (outside the post phase's own 10-90 band), the rest are settled.
        let values = [
            50.0, 52.0, 51.0, 30.0, 28.0, 10.0, 11.0, 10.0, 9.0, 10.0, 11.0, 10.0, 10.0,
        ];
        let fault = FaultWindow { start: 0, end: 3 };
        let p = plan(&values, fault).unwrap();
        assert_eq!(p.reference, Reference::PostBackcast);
        assert_eq!(
            p.context,
            vec![10.0, 10.0, 11.0, 10.0, 9.0, 10.0, 11.0, 10.0],
            "the recovering samples are not context"
        );
        assert_eq!(p.horizon, 5, "the backcast spans the skipped tail too");
        assert_eq!(p.covariates.past[0].len(), 8);
        assert_eq!(p.covariates.known_future[0].len(), 13);

        let impact = counterfactual(&Flat, &values, fault, 2).unwrap();
        assert_eq!(impact.forecast.horizon(), 3, "only the fault window");
        assert_eq!(impact.samples.len(), 13);
        assert_eq!(impact.summary.samples_during, 3);
        assert_eq!(impact.summary.samples_post, 10);
        assert_eq!(impact.recovery.recovered_at, Some(5));
        assert_eq!(impact.recovery.steps_after_fault, Some(2));
    }

    /// The samples of a live run: the probe recovers over two samples, then
    /// one stray spike follows on an otherwise settled post phase.
    fn run_with_a_stray_spike() -> Vec<f64> {
        let mut values = vec![53.0, 304.0, 308.0, 306.0];
        values.extend([167.0, 154.0, 54.0, 587.0, f64::NAN, 55.0, 54.0, 58.0]);
        values.extend([53.0, 53.0, 53.0, 53.0, 53.0, 55.0, 53.0, 53.0]);
        values.extend([53.0, 53.0, 54.0, 59.0]);
        values
    }

    #[test]
    fn backcast_context_leaves_out_a_stray_spike_after_the_tail() {
        let values = run_with_a_stray_spike();
        let p = plan(&values, FaultWindow { start: 0, end: 4 }).unwrap();
        assert_eq!(
            p.horizon, 6,
            "the fault window and the two recovering samples"
        );
        assert_eq!(p.context.len(), 18);
        // Nearest the fault: 54, then the spike as a gap, then the real gap.
        assert!((p.context[17] - 54.0).abs() < 1e-12);
        assert!(p.context[16].is_nan(), "the spike is not context");
        assert!(p.context[15].is_nan());
        assert!(p.context.iter().all(|v| v.is_nan() || *v < 60.0));
        assert_eq!(p.context.iter().filter(|v| v.is_nan()).count(), 2);
    }

    #[test]
    fn a_stray_spike_does_not_bend_the_backcast() {
        let values = run_with_a_stray_spike();
        let fault = FaultWindow { start: 0, end: 4 };
        let impact = counterfactual(&HoltWinters::new(), &values, fault, 3).unwrap();
        let top = impact
            .samples
            .iter()
            .take(4)
            .map(|s| s.median)
            .fold(f64::MIN, f64::max);
        assert!(top < 100.0, "the counterfactual follows the spike: {top}");
        assert!(impact.summary.during_peak > 1.0, "{:?}", impact.summary);
        assert!(impact.summary.during_outside_ratio >= 0.75);
        // The spike is still observed and still counted against the band.
        assert!((impact.samples[7].observed - 587.0).abs() < 1e-12);
        assert!(impact.samples[7].impact > 1.0);
    }

    #[test]
    fn backcast_keeps_a_post_phase_too_short_to_judge() {
        let values = [9.0, 8.0, 7.0, 3.0, 4.0];
        let p = plan(&values, FaultWindow { start: 0, end: 2 }).unwrap();
        assert_eq!(p.context, vec![4.0, 3.0, 7.0]);
        assert_eq!(p.horizon, 2);
    }

    #[test]
    fn backcast_skips_at_most_the_earlier_half() {
        // A post phase that never settles: only its earlier half can go.
        let values = [9.0, 60.0, 50.0, 40.0, 30.0, 20.0, 10.0];
        let p = plan(&values, FaultWindow { start: 0, end: 1 }).unwrap();
        assert_eq!(p.context, vec![10.0, 20.0, 30.0]);
        assert_eq!(p.horizon, 4);
    }

    /// An HTTP status probe: 200 before and after, 500 while the fault is on.
    #[test]
    fn a_flat_probe_gets_a_finite_peak_and_a_degenerate_flag() {
        let mut values = vec![200.0; 20];
        values.extend([500.0, 500.0, 500.0]);
        values.extend([200.0; 6]);
        let fault = FaultWindow { start: 20, end: 23 };
        let impact = counterfactual(&HoltWinters::new(), &values, fault, 2).unwrap();
        assert!(impact.degenerate_band);
        assert!(impact.forecast.degenerate_band);
        // The reference never moved, so the band is the floor: 0.1 % of
        // 200 each way, and 500 is 1500 of those above it.
        assert!(
            (impact.summary.during_peak - 1_500.0).abs() < 1e-3,
            "{:?}",
            impact.summary
        );
        assert!((impact.summary.during_outside_ratio - 1.0).abs() < 1e-9);
        assert_eq!(impact.recovery.steps_after_fault, Some(0));
        assert!(impact.samples.iter().all(|s| s.impact.is_finite()));
    }

    /// No pre-fault samples and a post phase that is exactly constant: the
    /// backcast band and the post band both collapse.
    #[test]
    fn an_exactly_flat_post_phase_gets_a_floored_band() {
        let mut values = vec![1.0, 1.0, 1.0];
        values.extend([0.0; 9]);
        let fault = FaultWindow { start: 0, end: 3 };
        let impact = counterfactual(&HoltWinters::new(), &values, fault, 2).unwrap();
        assert_eq!(impact.reference, Reference::PostBackcast);
        assert!(impact.degenerate_band);
        let peak = impact.summary.during_peak;
        assert!(peak.is_finite() && peak > 1.0, "{:?}", impact.summary);
        let post = &impact.samples[3];
        assert!(post.upper > post.lower, "{post:?}");
        assert!(post.impact.abs() < 1e-9);
        assert_eq!(impact.recovery.steps_after_fault, Some(0));
    }

    #[test]
    fn a_band_with_width_is_not_degenerate() {
        let values = [10.0, 10.0, 10.0, 10.0, 14.0, 16.0, 13.0, 11.0, 10.0, 10.0];
        let impact = counterfactual(&Flat, &values, FaultWindow { start: 4, end: 7 }, 2).unwrap();
        assert!(!impact.degenerate_band);
        assert!(!impact.forecast.degenerate_band);
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

    /// Deterministic pseudo-noise in `[-1, 1)` with no season to absorb.
    fn jitter(n: usize) -> Vec<f64> {
        let mut state: u64 = 7;
        (0..n)
            .map(|_| {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                f64::from(u8::try_from((state >> 56) & 0xff).unwrap_or(0)) / 128.0 - 1.0
            })
            .collect()
    }

    /// A baseline of 10 (the provider's band is 8-12), a fault a thousand
    /// times that, and a post phase stuck at 13. A floor scaled by the
    /// whole series would be about 4.4 and call 13 recovered.
    #[test]
    fn a_fault_a_thousand_times_the_baseline_does_not_hide_a_shifted_post_phase() {
        let mut values: Vec<f64> = jitter(40).iter().map(|j| 10.0 + 0.4 * j).collect();
        values.extend([10_000.0; 40]);
        values.extend(jitter(12).iter().map(|j| 13.0 + 0.4 * j));
        let fault = FaultWindow { start: 40, end: 80 };
        let impact = counterfactual(&Flat, &values, fault, 3).unwrap();
        assert_eq!(impact.reference, Reference::Baseline);
        assert!(!impact.degenerate_band);
        assert_eq!(impact.recovery.recovered_at, None, "{:?}", impact.recovery);
        let post = &impact.samples[40];
        assert!((post.upper - 12.0).abs() < 1e-9, "{post:?}");
        assert!(impact.summary.post_mean > 1.0, "{:?}", impact.summary);
    }

    /// The same run without pre-fault samples: the post band is the post
    /// phase's own spread, not a share of the timeouts.
    #[test]
    fn a_backcast_floors_its_band_from_the_settled_post_phase() {
        let mut values = vec![30_000.0; 40];
        values.extend(jitter(24).iter().map(|j| 50.0 + 2.0 * j));
        let fault = FaultWindow { start: 0, end: 40 };
        let impact = counterfactual(&HoltWinters::new(), &values, fault, 3).unwrap();
        assert_eq!(impact.reference, Reference::PostBackcast);
        let post = &impact.samples[60];
        assert!(post.upper - post.lower < 6.0, "{post:?}");
        let during = &impact.samples[10];
        assert!(during.upper - during.lower < 30.0, "{during:?}");
        assert!(impact.summary.during_peak > 1_000.0, "{:?}", impact.summary);
    }

    /// An 8e9 gauge moving by ± 1e6 that settles 5e6 higher has not
    /// recovered, though 5e6 is well inside 0.1 % of its level.
    #[test]
    fn a_large_offset_gauge_keeps_a_band_of_its_own_spread() {
        let mut values: Vec<f64> = jitter(40).iter().map(|j| 8e9 + 1e6 * j).collect();
        values.extend([8.1e9, 8.1e9, 8.1e9]);
        values.extend(jitter(10).iter().map(|j| 8e9 + 5e6 + 1e6 * j));
        let fault = FaultWindow { start: 40, end: 43 };
        let impact = counterfactual(&HoltWinters::new(), &values, fault, 3).unwrap();
        assert_eq!(impact.recovery.recovered_at, None, "{:?}", impact.recovery);
        assert!(impact.summary.post_mean > 1.0, "{:?}", impact.summary);
    }

    /// An integer metric whose later half happens to be exactly constant:
    /// 52, 54 and 55 next to 53 are jitter, not strays.
    #[test]
    fn a_flat_later_half_does_not_gap_ordinary_jitter() {
        let mut values = vec![300.0, 310.0, 305.0];
        values.extend([53.0, 52.0, 54.0, 55.0, 53.0, 54.0, 52.0, 53.0]);
        values.extend([53.0; 8]);
        let p = plan(&values, FaultWindow { start: 0, end: 3 }).unwrap();
        assert_eq!(p.horizon, 3, "nothing is still recovering");
        assert_eq!(p.context.len(), 16);
        assert!(p.context.iter().all(|v| v.is_finite()), "{:?}", p.context);
    }

    /// A value far from the rest that keeps coming back is a level of the
    /// metric, not a stray: gapping it would rewrite the phase.
    #[test]
    fn far_values_that_are_common_are_not_gapped() {
        let mut values = vec![300.0, 310.0];
        values.extend([10.0, 10.0, 90.0, 10.0, 10.0, 90.0, 10.0, 90.0, 10.0, 10.0]);
        values.extend([10.0; 10]);
        let p = plan(&values, FaultWindow { start: 0, end: 2 }).unwrap();
        assert_eq!(p.context.len(), 20);
        assert!(p.context.iter().all(|v| v.is_finite()), "{:?}", p.context);
    }

    /// One stray on an exactly flat phase is still left out, however short
    /// the phase.
    #[test]
    fn a_single_stray_on_a_flat_phase_is_still_a_gap() {
        let mut values = vec![300.0, 310.0];
        values.extend([53.0, 587.0, 53.0, 53.0, 53.0, 53.0, 53.0, 53.0]);
        let p = plan(&values, FaultWindow { start: 0, end: 2 }).unwrap();
        assert_eq!(p.context.len(), 8);
        assert_eq!(p.context.iter().filter(|v| v.is_nan()).count(), 1);
        assert!(p.context[6].is_nan(), "{:?}", p.context);
    }

    /// The post band is floored, so the head is judged against the floored
    /// band too: 53.04 on a phase that settles at exactly 53 is settled.
    #[test]
    fn the_unsettled_head_is_judged_against_the_floored_band() {
        let mut values = vec![300.0, 310.0];
        values.extend([53.04, 53.0, 53.0, 53.0, 53.0, 53.0, 53.0, 53.0]);
        let p = plan(&values, FaultWindow { start: 0, end: 2 }).unwrap();
        assert_eq!(p.horizon, 2, "no sample is skipped");
        assert_eq!(p.context.len(), 8);
    }
}
