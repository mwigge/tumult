//! `GET /api/forecast` and `GET /api/runs/{id}/impact` — the forecasting
//! endpoints.
//!
//! Both pull a series from the lake, resample it onto the series contract
//! (regular step, `NaN` gaps, capped) and ask a provider for quantile
//! paths. The provider is the Timestral sidecar when `TUMULTD_FORECAST_URL`
//! is set and answers; otherwise — unset, unreachable, or a bad answer —
//! the built-in Holt-Winters provider, and the response's `provenance`
//! says which one it was and why. A failing sidecar opens a circuit
//! breaker ([`breaker`]) so the requests that follow skip it and get the
//! fallback at once until a cool-down has passed. Metric forecasts are
//! cached in the lake's `forecasts` table with that provenance, keyed by
//! series, model, step and horizon, and reused while the context window
//! has not moved.

pub mod breaker;
pub mod client;

use std::collections::BTreeMap;
use std::time::Instant;

use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_forecast::impact::{self, FaultWindow, Impact};
use tumult_forecast::resample::{resample, step_for_window, RegularSeries};
use tumult_forecast::{
    Covariates, Forecast, ForecastError, ForecastProvider, ForecastRequest, HoltWinters,
    DEFAULT_LEVELS,
};
use tumult_lake::{ForecastRow, ModelFilter};

use crate::auth::Principal;
use crate::error::{bad_request, forbidden, not_found, ApiError};
use crate::sql_util::{env_table_predicate, internal, now_ns, sql_string, with_reader};
use crate::ApiState;

const DEFAULT_HORIZON: usize = 24;
const MAX_HORIZON: usize = 1_024;
const SECOND_NS: i64 = 1_000_000_000;
/// Observed steps in a row inside the band before a run counts as recovered.
const STABLE_STEPS: usize = 3;

/// `1h` / `6h` / `24h` / `7d` / `14d` / `30d` / `90d` → seconds.
fn parse_forecast_range(range: &str) -> Option<i64> {
    match range {
        "1h" => Some(3_600),
        "6h" => Some(6 * 3_600),
        "24h" => Some(86_400),
        "7d" => Some(7 * 86_400),
        "14d" => Some(14 * 86_400),
        "30d" => Some(30 * 86_400),
        "90d" => Some(90 * 86_400),
        _ => None,
    }
}

/// A forecast plus where it came from.
pub(crate) struct Produced {
    pub forecast: Forecast,
    /// `None` when the chosen provider answered; the sidecar's error when
    /// the built-in provider stepped in.
    pub fallback_reason: Option<String>,
}

/// Ask the sidecar unless its circuit is open; the error is the reason
/// the built-in provider has to step in.
async fn ask_sidecar(
    state: &ApiState,
    url: &str,
    context: &[f64],
    horizon: usize,
    covariates: Option<&Covariates>,
) -> Result<Forecast, String> {
    let breaker = state.forecast_breaker();
    if let Some(reason) = breaker.open_reason(Instant::now()) {
        return Err(reason);
    }
    let answer = client::TimestralClient::new(url)
        .forecast(context, horizon, &DEFAULT_LEVELS, covariates)
        .await;
    match answer {
        Ok(forecast) => {
            breaker.record_success();
            Ok(forecast)
        }
        Err(e) => {
            tracing::warn!(error = %e, unavailable = e.unavailable, "forecast sidecar failed; using the built-in provider");
            if e.unavailable {
                breaker.record_failure(Instant::now(), &e.message);
            }
            Err(e.message)
        }
    }
}

/// Ask the configured provider, falling back to Holt-Winters.
pub(crate) async fn produce(
    state: &ApiState,
    context: &[f64],
    horizon: usize,
    covariates: Option<&Covariates>,
) -> Result<Produced, ForecastError> {
    let mut fallback_reason = None;
    if let Some(url) = state.forecast_url() {
        match ask_sidecar(state, url, context, horizon, covariates).await {
            Ok(forecast) => {
                return Ok(Produced {
                    forecast,
                    fallback_reason: None,
                })
            }
            Err(reason) => fallback_reason = Some(reason),
        }
    }
    let forecast = HoltWinters::new().forecast(&ForecastRequest {
        context,
        horizon,
        levels: &DEFAULT_LEVELS,
        covariates,
    })?;
    Ok(Produced {
        forecast,
        fallback_reason,
    })
}

const BUILT_IN_MODEL: &str = "holt-winters";

/// Why the configured sidecar is being skipped right now, if it is.
fn sidecar_skip_reason(state: &ApiState) -> Option<String> {
    state.forecast_url()?;
    state.forecast_breaker().open_reason(Instant::now())
}

/// The rows a cache lookup accepts: the sidecar's (it names its own
/// model) while it is in use, the built-in provider's otherwise.
fn cache_filter(sidecar_in_use: bool) -> ModelFilter<'static> {
    if sidecar_in_use {
        ModelFilter::IsNot(BUILT_IN_MODEL)
    } else {
        ModelFilter::Is(BUILT_IN_MODEL)
    }
}

fn nullable(values: &[f64]) -> Vec<Value> {
    values
        .iter()
        .map(|v| if v.is_finite() { json!(v) } else { Value::Null })
        .collect()
}

fn number(v: f64) -> Value {
    if v.is_finite() {
        json!(v)
    } else {
        Value::Null
    }
}

fn provenance_json(forecast: &Forecast, fallback_reason: Option<&str>) -> Value {
    json!({
        "model": forecast.provenance.model,
        "version": forecast.provenance.version,
        "weights_hash": forecast.provenance.weights_hash,
        "fallback_reason": fallback_reason,
    })
}

/// Observed context followed by the horizon as one chart-ready series:
/// `ts` (ms) across both, `observed` null over the horizon, `median` and the
/// 10–90 band null over the context.
fn chart_series(series: &RegularSeries, forecast: &Forecast, offset: usize) -> Value {
    let n = series.len();
    let horizon = forecast.horizon();
    let ts: Vec<i64> = (0..offset + horizon)
        .map(|i| series.ts_ns(i) / 1_000_000)
        .collect();
    let mut observed = nullable(&series.values[..offset.min(n)]);
    observed.resize(offset + horizon, Value::Null);
    let pad = |values: Vec<f64>| -> Vec<Value> {
        let mut out = vec![Value::Null; offset];
        out.extend(nullable(&values));
        out
    };
    let (lower, upper) = forecast.band(0.1, 0.9).unwrap_or_default();
    json!({
        "ts": ts,
        "observed": observed,
        "median": pad(forecast.median()),
        "bands": [{"level": 0.8, "lower": pad(lower), "upper": pad(upper)}],
    })
}

// ---------------------------------------------------------------------------
// GET /api/forecast

#[derive(Deserialize)]
pub(crate) struct ForecastQuery {
    metric: Option<String>,
    target: Option<String>,
    env: Option<String>,
    range: Option<String>,
    horizon: Option<usize>,
}

/// Predicate confining a metric definition's source rows to one target:
/// `spans` carry `target_system`; the metric tables reach it through the
/// root span's `experiment_name`.
fn target_table_predicate(table: &str, target: &str) -> String {
    let t = sql_string(target);
    if table == "spans" {
        return format!("target_system = {t}");
    }
    format!(
        "EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = {table}.experiment_name \
         AND se.target_system = {t})"
    )
}

fn forecast_from_row(row: &ForecastRow) -> Forecast {
    Forecast {
        levels: row.quantile_levels.clone(),
        quantiles: row.quantiles.clone(),
        context_points: usize::try_from(row.context_points).unwrap_or_default(),
        insufficient_history: row.insufficient_history,
        provenance: tumult_forecast::Provenance {
            model: row.model.clone(),
            version: row.model_version.clone(),
            weights_hash: row.weights_hash.clone(),
        },
    }
}

fn row_from_forecast(
    key: &str,
    series: &RegularSeries,
    forecast: &Forecast,
    created_at_ns: i64,
) -> ForecastRow {
    ForecastRow {
        series_key: key.to_string(),
        model: forecast.provenance.model.clone(),
        model_version: forecast.provenance.version.clone(),
        weights_hash: forecast.provenance.weights_hash.clone(),
        context_start_ns: series.start_ns,
        context_end_ns: series.end_ns(),
        context_points: i64::try_from(forecast.context_points).unwrap_or_default(),
        insufficient_history: forecast.insufficient_history,
        horizon: i64::try_from(forecast.horizon()).unwrap_or_default(),
        step_ns: series.step_ns,
        quantile_levels: forecast.levels.clone(),
        quantiles: forecast.quantiles.clone(),
        created_at_ns,
    }
}

/// `GET /api/forecast?metric=&target=&env=&range=&horizon=`.
#[allow(clippy::too_many_lines)] // one request pipeline, read top to bottom
pub(crate) async fn forecast(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(q): Query<ForecastQuery>,
) -> Result<Json<Value>, ApiError> {
    let Some(metric) = q.metric.filter(|m| !m.is_empty()) else {
        return Err(bad_request("missing query parameter: metric"));
    };
    let range = q.range.unwrap_or_else(|| "24h".into());
    let Some(secs) = parse_forecast_range(&range) else {
        return Err(bad_request(format!(
            "invalid range {range:?}; expected 1h|6h|24h|7d|14d|30d|90d"
        )));
    };
    let horizon = q.horizon.unwrap_or(DEFAULT_HORIZON);
    if !(1..=MAX_HORIZON).contains(&horizon) {
        return Err(bad_request(format!(
            "horizon must be between 1 and {MAX_HORIZON} steps"
        )));
    }
    let envs: Vec<String> = match q.env.filter(|e| !e.is_empty()) {
        Some(env) => {
            if !principal.env_allowed(&env) {
                return Err(forbidden(format!(
                    "environment {env} is outside the principal's scopes"
                )));
            }
            vec![env]
        }
        None => principal.env_scopes.clone(),
    };
    let target = q.target.filter(|t| !t.is_empty());

    let defs = tumult_metrics::load_dir(&state.metrics_dir)
        .map_err(|e| internal(format!("load metrics: {e}")))?;
    let Some(def) = defs.iter().find(|d| d.name == metric) else {
        return Err(not_found(format!("metric {metric:?} not found")));
    };
    let to_ns = now_ns();
    let step_ns = step_for_window(secs * SECOND_NS);
    // Align the window to the step so the cache key is stable within a step.
    let to_ns = (to_ns / step_ns) * step_ns;
    let from_ns = to_ns - secs * SECOND_NS;
    let sql = tumult_metrics::to_sql_bucketed(def, step_ns, &[], Some((from_ns, to_ns)))
        .map_err(|e| internal(e.to_string()))?;
    let mut preds: Vec<String> = Vec::new();
    if let Some(env) = env_table_predicate(&def.source_table, &envs) {
        preds.push(env);
    }
    if let Some(t) = &target {
        preds.push(target_table_predicate(&def.source_table, t));
    }
    let sql = if preds.is_empty() {
        sql
    } else {
        sql.replacen(
            &format!("FROM {}", def.source_table),
            &format!(
                "FROM (SELECT * FROM {} WHERE {})",
                def.source_table,
                preds.join(" AND ")
            ),
            1,
        )
    };
    let key = format!(
        "metric={metric};target={};env={}",
        target.as_deref().unwrap_or(""),
        envs.join(",")
    );
    let description = def.description.clone();
    let skip_reason = sidecar_skip_reason(&state);
    let model = cache_filter(state.forecast_url().is_some() && skip_reason.is_none());
    let key2 = key.clone();
    let horizon_i64 = i64::try_from(horizon).unwrap_or_default();
    let (series, cached) = with_reader(&state.db_path, move |reader| {
        let rows = reader.query_json_rows(&sql).map_err(|e| e.to_string())?;
        let points: Vec<(i64, f64)> = rows
            .iter()
            .filter_map(|r| {
                let bucket_s = r.get("bucket_s")?.as_i64()?;
                let v = r.get("value")?.as_f64()?;
                Some((bucket_s * SECOND_NS, v))
            })
            .collect();
        let series = resample(&points, from_ns, to_ns, step_ns);
        let cached = reader
            .latest_forecast(&key2, model, step_ns, horizon_i64)
            .map_err(|e| e.to_string())?
            .filter(|row| row.context_end_ns == series.end_ns());
        Ok((series, cached))
    })
    .await?;

    let (forecast, fallback_reason, from_cache) = match cached {
        Some(row) => (forecast_from_row(&row), skip_reason, true),
        None => {
            if series.values.iter().all(|v| v.is_nan()) {
                return Err(not_found(format!(
                    "no points for metric {metric:?} in the last {range}"
                )));
            }
            let produced = produce(&state, &series.values, horizon, None)
                .await
                .map_err(|e| internal(e.to_string()))?;
            if let Some(ingest) = state.ingest_handle() {
                let row = row_from_forecast(&key, &series, &produced.forecast, now_ns());
                ingest
                    .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
                        writer.insert_forecast(&row).map_err(|e| e.to_string())
                    })))
                    .await
                    .map_err(|e| internal(e.to_string()))?;
            }
            (produced.forecast, produced.fallback_reason, false)
        }
    };
    let body = json!({
        "metric": metric,
        "description": description,
        "target": target,
        "env": envs,
        "range": range,
        "horizon": horizon,
        "step_ns": step_ns,
        "from_ns": from_ns,
        "to_ns": to_ns,
        "series": chart_series(&series, &forecast, series.len()),
        "forecast": {
            "levels": forecast.levels,
            "quantiles": forecast.quantiles.iter().map(|row| nullable(row)).collect::<Vec<_>>(),
            "context_points": forecast.context_points,
            "insufficient_history": forecast.insufficient_history,
        },
        "provenance": provenance_json(&forecast, fallback_reason.as_deref()),
        "cached": from_cache,
    });
    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// GET /api/runs/{id}/impact

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

fn impact_json(
    probe: &str,
    series: &RegularSeries,
    impact: &Impact,
    fallback: Option<&str>,
) -> Value {
    let n = series.len();
    let first = impact.samples.first().map_or(n, |s| s.index);
    let mut impacts = vec![Value::Null; first];
    impacts.extend(impact.samples.iter().map(|s| number(s.impact)));
    let mut median = vec![Value::Null; first];
    let mut lower = vec![Value::Null; first];
    let mut upper = vec![Value::Null; first];
    for s in &impact.samples {
        median.push(number(s.median));
        lower.push(number(s.lower));
        upper.push(number(s.upper));
    }
    let fault_start_ns = series.ts_ns(impact.fault.start);
    let fault_end_ns = series.ts_ns(impact.fault.end);
    let recovered_at_ns = impact
        .recovery
        .recovered_at
        .map(|i| series.ts_ns(first + i));
    json!({
        "probe": probe,
        "reference": impact.reference,
        "step_ns": series.step_ns,
        "fault": {"start_ns": fault_start_ns, "end_ns": fault_end_ns},
        "series": {
            "ts": series.ts_ms(),
            "observed": nullable(&series.values),
            "median": median,
            "bands": [{"level": 0.8, "lower": lower, "upper": upper}],
        },
        "impact": {
            "per_sample": impacts,
            "during_mean": number(impact.summary.during_mean),
            "during_peak": number(impact.summary.during_peak),
            "during_peak_ts_ms": impact.summary.during_peak_index.map(|i| series.ts_ns(i) / 1_000_000),
            "during_outside_ratio": number(impact.summary.during_outside_ratio),
            "post_mean": number(impact.summary.post_mean),
            "samples_during": impact.summary.samples_during,
            "samples_post": impact.summary.samples_post,
        },
        "recovery": {
            "recovered_at_ns": recovered_at_ns,
            "recovery_time_s": impact.recovery.seconds(series.step_ns),
            "stable_steps": impact.recovery.stable_steps,
            "outside_during_fault": impact.recovery.outside_during_fault,
            "outside_after_fault": impact.recovery.outside_after_fault,
        },
        "forecast": {
            "context_points": impact.forecast.context_points,
            "insufficient_history": impact.forecast.insufficient_history,
        },
        "provenance": provenance_json(&impact.forecast, fallback),
    })
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
    let scopes = principal.env_scopes.clone();
    let lookup = id.clone();
    let (experiment_id, rows) = with_reader(&state.db_path, move |reader| {
        let scope_predicate = crate::auth::scopes::run_scope_sql(&scopes);
        let run = reader
            .query_json_rows(&format!(
                "SELECT r.experiment_id FROM runs r \
                 LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env \
                            FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id \
                 WHERE r.id = {} AND ({scope_predicate})",
                sql_string(&lookup)
            ))
            .map_err(|e| e.to_string())?
            .into_iter()
            .next();
        let Some(run) = run else {
            return Ok((None, Vec::new()));
        };
        let Some(experiment_id) = run
            .get("experiment_id")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return Ok((Some(String::new()), Vec::new()));
        };
        let rows = reader
            .query_json_rows(&format!(
                "SELECT probe_name, phase, ts_ns, value FROM probe_samples \
                 WHERE experiment_id = {} ORDER BY ts_ns ASC",
                sql_string(&experiment_id)
            ))
            .map_err(|e| e.to_string())?;
        Ok((Some(experiment_id), rows))
    })
    .await?;
    let Some(experiment_id) = experiment_id else {
        return Err(not_found(format!("unknown run id {id:?}")));
    };
    if rows.is_empty() {
        return Err(not_found(format!("run {id:?} has no probe samples")));
    }
    let mut probes = Vec::new();
    let mut worst_recovery_s: Option<f64> = None;
    let mut worst_peak: Option<f64> = None;
    for (name, samples) in group_samples(&rows) {
        let Some((series, fault)) = probe_series(&samples) else {
            probes.push(json!({"probe": name, "error": "no during-phase samples"}));
            continue;
        };
        let plan = match impact::plan(&series.values, fault) {
            Ok(plan) => plan,
            Err(e) => {
                probes.push(json!({"probe": name, "error": e.to_string()}));
                continue;
            }
        };
        let produced = produce(&state, &plan.context, plan.horizon, Some(&plan.covariates))
            .await
            .map_err(|e| internal(e.to_string()))?;
        let impact = plan
            .evaluate(produced.forecast, &series.values, STABLE_STEPS)
            .map_err(|e| internal(e.to_string()))?;
        if let Some(s) = impact.recovery.seconds(series.step_ns) {
            worst_recovery_s = Some(worst_recovery_s.map_or(s, |w| w.max(s)));
        }
        if impact.summary.during_peak.is_finite() {
            let p = impact.summary.during_peak.abs();
            worst_peak = Some(worst_peak.map_or(p, |w| w.max(p)));
        }
        probes.push(impact_json(
            &name,
            &series,
            &impact,
            produced.fallback_reason.as_deref(),
        ));
    }
    let provenance = probes
        .iter()
        .find_map(|p| p.get("provenance").cloned())
        .unwrap_or(Value::Null);
    Ok(Json(json!({
        "run_id": id,
        "experiment_id": experiment_id,
        "as_of_ns": now_ns(),
        "stable_steps": STABLE_STEPS,
        "recovery_time_s": worst_recovery_s,
        "impact_peak": worst_peak,
        "provenance": provenance,
        "probes": probes,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_and_target_predicates() {
        assert_eq!(parse_forecast_range("1h"), Some(3_600));
        assert_eq!(parse_forecast_range("90d"), Some(90 * 86_400));
        assert_eq!(parse_forecast_range("2w"), None);
        assert_eq!(
            target_table_predicate("spans", "o'db"),
            "target_system = 'o''db'"
        );
        assert!(target_table_predicate("metric_sums", "db").contains("metric_sums.experiment_name"));
    }

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

    #[test]
    fn chart_series_pads_context_and_horizon() {
        let series = RegularSeries {
            start_ns: 0,
            step_ns: SECOND_NS,
            values: vec![1.0, f64::NAN],
        };
        let forecast = HoltWinters::new()
            .forecast(&ForecastRequest::new(&series.values, 2))
            .unwrap();
        let v = chart_series(&series, &forecast, 2);
        assert_eq!(v["ts"].as_array().unwrap().len(), 4);
        assert_eq!(v["observed"][0], 1.0);
        assert!(v["observed"][1].is_null() && v["observed"][2].is_null());
        assert!(v["median"][0].is_null());
        assert!(v["median"][2].is_number());
        assert_eq!(v["bands"][0]["level"], 0.8);
    }
}
