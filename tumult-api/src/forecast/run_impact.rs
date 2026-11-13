//! `GET /api/runs/{id}/impact` — what a run's fault did to each probe,
//! against the counterfactual a provider forecasts (forward from the
//! pre-fault samples, or backwards from the settled post phase).
//!
//! One probe failing — no fault window, too little history, a forecast
//! that cannot be evaluated — is that probe's `error` entry; the others
//! are still answered and the request is never a 500 for it.

use std::collections::BTreeMap;

use axum::extract::{Path, State};
use axum::{Extension, Json};
use serde_json::{json, Value};
use tumult_forecast::impact::{self, FaultWindow, Impact};
use tumult_forecast::recovery::Verdict;
use tumult_forecast::resample::{resample, step_for_window, RegularSeries};
use tumult_forecast::ForecastError;

use super::{nullable, number, produce, provenance_json, MS_NS, SECOND_NS};
use crate::auth::Principal;
use crate::error::{bad_request, not_found, ApiError};
use crate::sql_util::{now_ns, sql_string, with_reader};
use crate::ApiState;

/// Observed steps in a row inside the band before a run counts as recovered.
const STABLE_STEPS: usize = 3;

/// Raw samples of one probe, in time order.
#[derive(Debug, Default)]
struct ProbeSamples {
    points: Vec<(i64, f64)>,
    during: Vec<i64>,
}

/// Group `probe_samples` rows by probe name.
fn group_samples(rows: &[Value]) -> BTreeMap<String, ProbeSamples> {
    let mut by_probe: BTreeMap<String, ProbeSamples> = BTreeMap::new();
    for r in rows {
        let (Some(name), Some(ts), Some(value)) = (
            r.get("probe_name").and_then(Value::as_str),
            r.get("ts_ns").and_then(Value::as_i64),
            r.get("value").and_then(Value::as_f64),
        ) else {
            continue;
        };
        let entry = by_probe.entry(name.to_string()).or_default();
        entry.points.push((ts, value));
        if r.get("phase").and_then(Value::as_str) == Some("during") {
            entry.during.push(ts);
        }
    }
    by_probe
}

/// The series step for a probe: the window's tier, or the probe's own
/// sampling interval (the median gap between samples) when that is coarser,
/// so a 5 s sampler does not become a series that is four-fifths gaps.
fn sample_step_ns(points: &[(i64, f64)], window_ns: i64) -> i64 {
    let mut gaps: Vec<i64> = points
        .windows(2)
        .map(|w| w[1].0 - w[0].0)
        .filter(|g| *g > 0)
        .collect();
    gaps.sort_unstable();
    let median_gap = gaps.get(gaps.len() / 2).copied().unwrap_or(0);
    let rounded = (median_gap / SECOND_NS) * SECOND_NS;
    step_for_window(window_ns).max(rounded)
}

/// Resample one probe and locate its fault window; `None` when the probe
/// has no during-phase samples.
fn probe_series(samples: &ProbeSamples) -> Option<(RegularSeries, FaultWindow)> {
    let first = samples.points.first()?.0;
    let last = samples.points.last()?.0;
    let during_start = *samples.during.iter().min()?;
    let during_end = *samples.during.iter().max()?;
    let step_ns = sample_step_ns(&samples.points, last - first);
    let series = resample(&samples.points, first, last + step_ns, step_ns);
    let start = series.index_of(during_start)?;
    let end = series.index_of(during_end)? + 1;
    Some((series, FaultWindow { start, end }))
}

/// The counterfactual's median and band, padded with nulls before the
/// first compared step so they line up with the observed series.
fn band_series(series: &RegularSeries, impact: &Impact, first: usize) -> Value {
    let mut median = vec![Value::Null; first];
    let mut lower = vec![Value::Null; first];
    let mut upper = vec![Value::Null; first];
    for s in &impact.samples {
        median.push(number(s.median));
        lower.push(number(s.lower));
        upper.push(number(s.upper));
    }
    json!({
        "ts": series.ts_ms(),
        "observed": nullable(&series.values),
        "median": median,
        "bands": [{"level": 0.8, "lower": lower, "upper": upper}],
    })
}

fn summary_json(series: &RegularSeries, impact: &Impact, first: usize) -> Value {
    let mut per_sample = vec![Value::Null; first];
    per_sample.extend(impact.samples.iter().map(|s| number(s.impact)));
    let summary = &impact.summary;
    json!({
        "per_sample": per_sample,
        "during_mean": number(summary.during_mean),
        "during_peak": number(summary.during_peak),
        "during_peak_ts_ms": summary.during_peak_index.map(|i| series.ts_ns(i) / MS_NS),
        "during_outside_ratio": number(summary.during_outside_ratio),
        "post_mean": number(summary.post_mean),
        "samples_during": summary.samples_during,
        "samples_post": summary.samples_post,
    })
}

fn recovery_json(series: &RegularSeries, impact: &Impact, first: usize) -> Value {
    let recovery = &impact.recovery;
    let recovered_at_ns = recovery.recovered_at.map(|i| series.ts_ns(first + i));
    json!({
        "recovered_at_ns": recovered_at_ns,
        "recovered_at_ms": recovered_at_ns.map(|ns| ns / MS_NS),
        "recovery_time_s": recovery.seconds(series.step_ns),
        "status": recovery.verdict(),
        "stable_steps": recovery.stable_steps,
        "outside_during_fault": recovery.outside_during_fault,
        "outside_after_fault": recovery.outside_after_fault,
        "observed_after_fault": recovery.observed_after_fault,
        "ended_inside": recovery.ended_inside,
    })
}

fn impact_json(
    probe: &str,
    series: &RegularSeries,
    impact: &Impact,
    fallback: Option<&str>,
) -> Value {
    let first = impact.samples.first().map_or(series.len(), |s| s.index);
    let fault_start_ns = series.ts_ns(impact.fault.start);
    let fault_end_ns = series.ts_ns(impact.fault.end);
    json!({
        "probe": probe,
        "reference": impact.reference,
        "step_ns": series.step_ns,
        "degenerate_band": impact.degenerate_band,
        "fault": {
            "start_ns": fault_start_ns,
            "end_ns": fault_end_ns,
            "start_ms": fault_start_ns / MS_NS,
            "end_ms": fault_end_ns / MS_NS,
        },
        "series": band_series(series, impact, first),
        "impact": summary_json(series, impact, first),
        "recovery": recovery_json(series, impact, first),
        "forecast": {
            "context_points": impact.forecast.context_points,
            "insufficient_history": impact.forecast.insufficient_history,
            "degenerate_band": impact.forecast.degenerate_band,
        },
        "provenance": provenance_json(&impact.forecast, fallback),
    })
}

/// One probe's answer and what it adds to the run's headline.
struct ProbeImpact {
    json: Value,
    verdict: Verdict,
    recovery_s: Option<f64>,
    peak: Option<f64>,
    degenerate_band: bool,
}

/// The counterfactual for one probe. The error is the probe's own: the
/// caller reports it next to the probes that worked.
async fn probe_impact(
    state: &ApiState,
    run_id: &str,
    name: &str,
    samples: &ProbeSamples,
) -> Result<ProbeImpact, String> {
    let (series, fault) = probe_series(samples).ok_or("no during-phase samples")?;
    let plan = impact::plan(&series.values, fault).map_err(|e| e.to_string())?;
    // What a busy sidecar's `Retry-After` is remembered against.
    let held_as = format!("impact\u{1f}{run_id}\u{1f}{name}");
    let covariates = Some(&plan.covariates);
    let produced = produce(state, &held_as, &plan.context, plan.horizon, covariates)
        .await
        .map_err(|e| e.to_string())?;
    let forecast = produced.forecast;
    let (series, impact) = tokio::task::spawn_blocking(move || {
        let impact = plan.evaluate(forecast, &series.values, STABLE_STEPS)?;
        Ok::<_, ForecastError>((series, impact))
    })
    .await
    .map_err(|e| format!("the evaluation did not finish: {e}"))?
    .map_err(|e| e.to_string())?;
    let peak = impact.summary.during_peak;
    Ok(ProbeImpact {
        json: impact_json(name, &series, &impact, produced.fallback_reason),
        verdict: impact.recovery.verdict(),
        recovery_s: impact.recovery.seconds(series.step_ns),
        peak: peak.is_finite().then(|| peak.abs()),
        degenerate_band: impact.degenerate_band,
    })
}

fn larger(worst: Option<f64>, candidate: Option<f64>) -> Option<f64> {
    match (worst, candidate) {
        (Some(w), Some(c)) => Some(w.max(c)),
        (w, c) => w.or(c),
    }
}

/// What the evaluated probes add up to for the run as a whole.
#[derive(Debug, Default)]
struct Headline {
    /// Probes whose impact could be evaluated.
    evaluated: usize,
    /// Of those, the ones that never settled back inside their band.
    unrecovered: usize,
    /// Of those, the ones not watched long enough after the fault to say.
    undetermined: usize,
    slowest_recovery_s: Option<f64>,
    worst_peak: Option<f64>,
    degenerate_band: bool,
}

/// What the run's recovery time means, said outright so that no consumer
/// has to work it out from the counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum RunRecovery {
    /// Every probe that was watched long enough settled.
    Recovered,
    /// At least one evaluated probe never settled.
    Unrecovered,
    /// Probes were evaluated, none recovered and none is unrecovered.
    Undetermined,
    /// No probe could be evaluated at all.
    NotMeasurable,
}

impl Headline {
    /// Which of the four the run is. `recovery_time_s` is `None` for three
    /// of them, and they are not the same finding.
    fn status(&self) -> RunRecovery {
        if self.evaluated == 0 {
            return RunRecovery::NotMeasurable;
        }
        if self.unrecovered > 0 {
            return RunRecovery::Unrecovered;
        }
        if self.slowest_recovery_s.is_some() {
            return RunRecovery::Recovered;
        }
        RunRecovery::Undetermined
    }

    fn add(&mut self, probe: &ProbeImpact) {
        self.evaluated += 1;
        match probe.verdict {
            Verdict::Recovered => {}
            Verdict::Unrecovered => self.unrecovered += 1,
            Verdict::Undetermined => self.undetermined += 1,
        }
        self.slowest_recovery_s = larger(self.slowest_recovery_s, probe.recovery_s);
        self.worst_peak = larger(self.worst_peak, probe.peak);
        self.degenerate_band |= probe.degenerate_band;
    }

    /// The run's recovery time: the slowest probe's, and `None` (not
    /// recovered) as soon as one evaluated probe never settled. A run is
    /// not back because most of it is. A probe that was not watched long
    /// enough to say is left out: it is no evidence either way, and the
    /// time is `None` only when no probe at all was seen to recover.
    fn recovery_time_s(&self) -> Option<f64> {
        if self.unrecovered > 0 {
            return None;
        }
        self.slowest_recovery_s
    }
}

/// The run's experiment and its probe samples in time order; `None` when
/// the run does not exist or is outside `scopes`.
async fn load_samples(
    state: &ApiState,
    run_id: String,
    scopes: Vec<String>,
) -> Result<Option<(String, Vec<Value>)>, ApiError> {
    with_reader(&state.db_path, move |reader| {
        let scope_predicate = crate::auth::scopes::run_scope_sql(&scopes);
        let run = reader
            .query_json_rows(&format!(
                "SELECT r.experiment_id FROM runs r \
                 LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env \
                            FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id \
                 WHERE r.id = {} AND ({scope_predicate})",
                sql_string(&run_id)
            ))
            .map_err(|e| e.to_string())?
            .into_iter()
            .next();
        let Some(run) = run else {
            return Ok(None);
        };
        let Some(experiment_id) = run.get("experiment_id").and_then(Value::as_str) else {
            return Ok(Some((String::new(), Vec::new())));
        };
        let rows = reader
            .query_json_rows(&format!(
                "SELECT probe_name, phase, ts_ns, value FROM probe_samples \
                 WHERE experiment_id = {} ORDER BY ts_ns ASC",
                sql_string(experiment_id)
            ))
            .map_err(|e| e.to_string())?;
        Ok(Some((experiment_id.to_owned(), rows)))
    })
    .await
}

/// `GET /api/runs/{id}/impact`.
pub(crate) async fn run_impact(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if id.chars().count() > 100 {
        return Err(bad_request("run id too long"));
    }
    let loaded = load_samples(&state, id.clone(), principal.env_scopes.clone()).await?;
    let Some((experiment_id, rows)) = loaded else {
        return Err(not_found(format!("unknown run id {id:?}")));
    };
    if rows.is_empty() {
        return Err(not_found(format!("run {id:?} has no probe samples")));
    }
    let mut probes = Vec::new();
    let mut headline = Headline::default();
    for (name, samples) in group_samples(&rows) {
        match probe_impact(&state, &id, &name, &samples).await {
            Ok(probe) => {
                headline.add(&probe);
                probes.push(probe.json);
            }
            Err(error) => probes.push(json!({"probe": name, "error": error})),
        }
    }
    let provenance = probes
        .iter()
        .find_map(|p| p.get("provenance").cloned())
        .unwrap_or(Value::Null);
    let as_of_ns = now_ns();
    Ok(Json(json!({
        "run_id": id,
        "experiment_id": experiment_id,
        "as_of_ns": as_of_ns,
        "as_of_ms": as_of_ns / MS_NS,
        "stable_steps": STABLE_STEPS,
        "recovery_time_s": headline.recovery_time_s(),
        "recovery_status": headline.status(),
        "unrecovered_probes": headline.unrecovered,
        "undetermined_probes": headline.undetermined,
        "evaluated_probes": headline.evaluated,
        "impact_peak": headline.worst_peak,
        "degenerate_band": headline.degenerate_band,
        "provenance": provenance,
        "probes": probes,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_series_uses_the_sampling_interval_and_finds_the_fault() {
        let s = SECOND_NS;
        let samples = ProbeSamples {
            points: vec![
                (0, 1.0),
                (5 * s, 2.0),
                (10 * s, 9.0),
                (15 * s, 1.0),
                (20 * s, 1.0),
            ],
            during: vec![5 * s, 10 * s],
        };
        let (series, fault) = probe_series(&samples).unwrap();
        assert_eq!(series.step_ns, 5 * s);
        assert_eq!(series.len(), 5);
        assert_eq!(fault, FaultWindow { start: 1, end: 3 });
        assert!(probe_series(&ProbeSamples::default()).is_none());
        let no_during = ProbeSamples {
            points: vec![(0, 1.0)],
            during: vec![],
        };
        assert!(probe_series(&no_during).is_none());
    }

    #[test]
    fn grouping_keeps_time_order_and_phases() {
        let rows = vec![
            json!({"probe_name": "b", "phase": "during", "ts_ns": 1, "value": 1.0}),
            json!({"probe_name": "a", "phase": "post", "ts_ns": 2, "value": 2.0}),
            json!({"probe_name": "b", "phase": "post", "ts_ns": 3, "value": 3.0}),
            json!({"probe_name": "b", "phase": "post", "ts_ns": "bad", "value": 3.0}),
        ];
        let g = group_samples(&rows);
        assert_eq!(g.keys().collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(g["b"].points.len(), 2);
        assert_eq!(g["b"].during, vec![1]);
        assert!(g["a"].during.is_empty());
    }

    fn probe(recovery_s: Option<f64>, peak: Option<f64>) -> ProbeImpact {
        let verdict = match recovery_s {
            Some(_) => Verdict::Recovered,
            None => Verdict::Unrecovered,
        };
        ProbeImpact {
            json: Value::Null,
            verdict,
            recovery_s,
            peak,
            degenerate_band: false,
        }
    }

    /// A probe nobody watched after the fault is no evidence either way:
    /// it neither nulls the time the watched probes took nor counts as one
    /// that did not come back.
    #[test]
    fn an_undetermined_probe_leaves_the_headline_alone() {
        let unwatched = ProbeImpact {
            verdict: Verdict::Undetermined,
            ..probe(None, Some(9.0))
        };
        let mut headline = Headline::default();
        headline.add(&unwatched);
        assert_eq!(headline.recovery_time_s(), None, "nothing was seen");
        headline.add(&probe(Some(4.0), Some(1.0)));
        assert_eq!(headline.recovery_time_s(), Some(4.0));
        assert_eq!(
            (
                headline.unrecovered,
                headline.undetermined,
                headline.evaluated
            ),
            (0, 1, 2)
        );
        assert_eq!(headline.worst_peak, Some(9.0), "its peak still counts");
        headline.add(&probe(None, None));
        assert_eq!(headline.recovery_time_s(), None);
    }

    /// The four things a run's recovery can be, each from its own counts.
    #[test]
    fn the_headline_names_its_recovery_state() {
        let unwatched = ProbeImpact {
            verdict: Verdict::Undetermined,
            ..probe(None, Some(9.0))
        };
        let mut headline = Headline::default();
        assert_eq!(headline.status(), RunRecovery::NotMeasurable);
        headline.add(&unwatched);
        assert_eq!(headline.status(), RunRecovery::Undetermined);
        headline.add(&probe(Some(4.0), None));
        assert_eq!(headline.status(), RunRecovery::Recovered);
        headline.add(&probe(None, None));
        assert_eq!(headline.status(), RunRecovery::Unrecovered);
        assert_eq!(
            serde_json::to_value(RunRecovery::NotMeasurable).unwrap(),
            json!("not_measurable")
        );
    }

    #[test]
    fn the_headline_is_the_slowest_recovery_and_the_largest_peak() {
        let mut headline = Headline::default();
        assert_eq!(headline.recovery_time_s(), None, "nothing was evaluated");
        headline.add(&probe(Some(3.0), Some(2.0)));
        headline.add(&probe(Some(1.0), None));
        headline.add(&probe(Some(2.0), Some(7.5)));
        assert_eq!(headline.recovery_time_s(), Some(3.0));
        assert_eq!(headline.worst_peak, Some(7.5));
        assert_eq!(headline.evaluated, 3);
        assert_eq!(headline.unrecovered, 0);
    }

    /// One probe that never came back is the run's answer, however fast
    /// the others settled, and whichever order they arrive in.
    #[test]
    fn the_headline_is_not_recovered_when_any_probe_did_not_recover() {
        for order in [[Some(4.0), None], [None, Some(4.0)]] {
            let mut headline = Headline::default();
            for recovery_s in order {
                headline.add(&probe(recovery_s, Some(1.0)));
            }
            assert_eq!(headline.recovery_time_s(), None, "{order:?}");
            assert_eq!(headline.unrecovered, 1);
            assert_eq!(headline.evaluated, 2);
        }
    }
}
