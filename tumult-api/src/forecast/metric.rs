//! `GET /api/forecast?metric=&target=&env=&range=&horizon=` — a semantic
//! metric's series and its forecast.
//!
//! Forecasts are cached in the lake's `forecasts` table with their
//! provenance. A row is reused only for the same series (metric, target,
//! environments), the same provider kind, step and horizon, and exactly
//! the same context window: a day and a fortnight share a step but not a
//! forecast. The window end is aligned to the step, and to a minute in
//! the one-second tier, so a chart that polls adds at most one row a
//! minute; rows older than [`FORECAST_RETENTION_NS`] go with each write.
//! The cache is an optimisation: a failed lookup is a miss and a failed
//! write is logged, and the forecast is served either way.

use axum::extract::{Query, State};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_forecast::resample::{resample, step_for_window, RegularSeries};
use tumult_forecast::Forecast;
use tumult_lake::{ForecastLookup, ForecastRow, ModelFilter, FORECAST_RETENTION_NS};
use tumult_metrics::MetricDef;

use super::{
    chart_series, nullable, produce, provenance_json, sidecar_skip_reason, telemetry,
    BUILT_IN_MODEL, MS_NS, SECOND_NS,
};
use crate::auth::Principal;
use crate::error::{bad_request, forbidden, not_found, ApiError};
use crate::sql_util::{env_table_predicate, internal, now_ns, sql_string, with_reader};
use crate::ApiState;

const DEFAULT_HORIZON: usize = 24;
const MAX_HORIZON: usize = 1_024;
/// The coarsest the window end moves in the one-second tier.
const CACHE_ALIGN_NS: i64 = 60 * SECOND_NS;

#[derive(Deserialize)]
pub(crate) struct ForecastQuery {
    metric: Option<String>,
    target: Option<String>,
    env: Option<String>,
    range: Option<String>,
    horizon: Option<usize>,
}

/// A validated request.
struct Asked {
    metric: String,
    target: Option<String>,
    envs: Vec<String>,
    range: String,
    secs: i64,
    horizon: usize,
}

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

/// The environments the request covers: the one asked for when the
/// principal may see it, the principal's scopes otherwise.
fn asked_envs(env: Option<String>, principal: &Principal) -> Result<Vec<String>, ApiError> {
    let Some(env) = env.filter(|e| !e.is_empty()) else {
        return Ok(principal.env_scopes.clone());
    };
    if !principal.env_allowed(&env) {
        return Err(forbidden(format!(
            "environment {env} is outside the principal's scopes"
        )));
    }
    Ok(vec![env])
}

fn asked(q: ForecastQuery, principal: &Principal) -> Result<Asked, ApiError> {
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
    Ok(Asked {
        metric,
        target: q.target.filter(|t| !t.is_empty()),
        envs: asked_envs(q.env, principal)?,
        range,
        secs,
        horizon,
    })
}

/// The context window on its step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Window {
    step_ns: i64,
    from_ns: i64,
    to_ns: i64,
}

/// The `secs` up to `now_ns`, with the end aligned to the step — and to
/// [`CACHE_ALIGN_NS`] when the step is finer — so the window, and with it
/// the cache entry, holds still between alignments.
fn window(secs: i64, now_ns: i64) -> Window {
    let step_ns = step_for_window(secs * SECOND_NS);
    let align = step_ns.max(CACHE_ALIGN_NS);
    let to_ns = (now_ns / align) * align;
    Window {
        step_ns,
        from_ns: to_ns - secs * SECOND_NS,
        to_ns,
    }
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

/// The metric's bucketed series over the window, confined to the asked
/// environments and target.
fn series_sql(def: &MetricDef, asked: &Asked, window: Window) -> Result<String, ApiError> {
    let range = Some((window.from_ns, window.to_ns));
    let sql = tumult_metrics::to_sql_bucketed(def, window.step_ns, &[], range)
        .map_err(|e| internal(e.to_string()))?;
    let mut preds: Vec<String> = Vec::new();
    if let Some(env) = env_table_predicate(&def.source_table, &asked.envs) {
        preds.push(env);
    }
    if let Some(t) = &asked.target {
        preds.push(target_table_predicate(&def.source_table, t));
    }
    if preds.is_empty() {
        return Ok(sql);
    }
    let table = &def.source_table;
    let confined = format!("FROM (SELECT * FROM {table} WHERE {})", preds.join(" AND "));
    Ok(sql.replacen(&format!("FROM {table}"), &confined, 1))
}

/// The cache key of a series. A JSON array, so a metric, target or
/// environment holding any separator cannot be read as another series.
fn series_key(asked: &Asked) -> String {
    json!([asked.metric, asked.target, asked.envs]).to_string()
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

fn forecast_from_row(row: &ForecastRow) -> Forecast {
    Forecast {
        levels: row.quantile_levels.clone(),
        quantiles: row.quantiles.clone(),
        context_points: usize::try_from(row.context_points).unwrap_or_default(),
        insufficient_history: row.insufficient_history,
        degenerate_band: row.degenerate_band,
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
        degenerate_band: forecast.degenerate_band,
        horizon: i64::try_from(forecast.horizon()).unwrap_or_default(),
        step_ns: series.step_ns,
        quantile_levels: forecast.levels.clone(),
        quantiles: forecast.quantiles.clone(),
        created_at_ns,
    }
}

/// `bucket_s`/`value` rows → `(ts_ns, value)` points.
fn points(rows: &[Value]) -> Vec<(i64, f64)> {
    rows.iter()
        .filter_map(|r| {
            let bucket_s = r.get("bucket_s")?.as_i64()?;
            let v = r.get("value")?.as_f64()?;
            Some((bucket_s * SECOND_NS, v))
        })
        .collect()
}

/// What a cache lookup needs besides the series itself.
struct CacheQuery {
    key: String,
    model: ModelFilter<'static>,
    horizon: usize,
}

/// The series over the window and the cached forecast fitted on exactly
/// that window, when there is one. A lookup that fails is a miss.
async fn load(
    state: &ApiState,
    sql: String,
    window: Window,
    cache: CacheQuery,
) -> Result<(RegularSeries, Option<ForecastRow>), ApiError> {
    with_reader(&state.db_path, move |reader| {
        let rows = reader.query_json_rows(&sql).map_err(|e| e.to_string())?;
        let series = resample(&points(&rows), window.from_ns, window.to_ns, window.step_ns);
        let lookup = ForecastLookup {
            series_key: &cache.key,
            model: cache.model,
            step_ns: series.step_ns,
            horizon: i64::try_from(cache.horizon).unwrap_or_default(),
            context_start_ns: series.start_ns,
            context_end_ns: series.end_ns(),
        };
        let cached = reader.latest_forecast(&lookup).unwrap_or_else(|e| {
            tracing::warn!(
                error.type = "cache_lookup",
                error.message = %e,
                "forecast cache lookup failed; fitting afresh"
            );
            None
        });
        Ok((series, cached))
    })
    .await
}

/// Cache `row` and drop the rows past their retention. Failing to is
/// logged, never the request's problem.
async fn store(state: &ApiState, row: ForecastRow) {
    let Some(ingest) = state.ingest_handle() else {
        return;
    };
    let cutoff_ns = row.created_at_ns - FORECAST_RETENTION_NS;
    let written = ingest
        .write(tumult_ingest::Batch::Exec(Box::new(move |writer| {
            writer.insert_forecast(&row).map_err(|e| e.to_string())?;
            writer
                .prune_forecasts(cutoff_ns)
                .map(|_| ())
                .map_err(|e| e.to_string())
        })))
        .await;
    if let Err(e) = written {
        tracing::warn!(
            error.type = "cache_write",
            error.message = %e,
            "forecast cache write failed; serving the forecast uncached"
        );
    }
}

/// A forecast ready to serve.
struct Answer {
    forecast: Forecast,
    fallback_reason: Option<&'static str>,
    cached: bool,
}

/// Fit a forecast for `series` and cache it.
async fn fresh(
    state: &ApiState,
    asked: &Asked,
    key: &str,
    series: &RegularSeries,
) -> Result<Answer, ApiError> {
    if !series.values.iter().any(|v| v.is_finite()) {
        return Err(not_found(format!(
            "no points for metric {:?} in the last {}",
            asked.metric, asked.range
        )));
    }
    let produced = produce(state, &series.values, asked.horizon, None)
        .await
        .map_err(|e| internal(e.to_string()))?;
    let row = row_from_forecast(key, series, &produced.forecast, now_ns());
    store(state, row).await;
    Ok(Answer {
        forecast: produced.forecast,
        fallback_reason: produced.fallback_reason,
        cached: false,
    })
}

fn body(
    asked: &Asked,
    description: Option<&str>,
    window: Window,
    series: &RegularSeries,
    answer: &Answer,
) -> Value {
    let forecast = &answer.forecast;
    json!({
        "metric": asked.metric,
        "description": description,
        "target": asked.target,
        "env": asked.envs,
        "range": asked.range,
        "horizon": asked.horizon,
        "step_ns": window.step_ns,
        "from_ns": window.from_ns,
        "to_ns": window.to_ns,
        "from_ms": window.from_ns / MS_NS,
        "to_ms": window.to_ns / MS_NS,
        "series": chart_series(series, forecast, series.len()),
        "forecast": {
            "levels": forecast.levels,
            "quantiles": forecast.quantiles.iter().map(|row| nullable(row)).collect::<Vec<_>>(),
            "context_points": forecast.context_points,
            "insufficient_history": forecast.insufficient_history,
            "degenerate_band": forecast.degenerate_band,
        },
        "provenance": provenance_json(forecast, answer.fallback_reason),
        "cached": answer.cached,
    })
}

/// `GET /api/forecast?metric=&target=&env=&range=&horizon=`.
pub(crate) async fn forecast(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(q): Query<ForecastQuery>,
) -> Result<Json<Value>, ApiError> {
    let asked = asked(q, &principal)?;
    let defs = tumult_metrics::load_dir(&state.metrics_dir)
        .map_err(|e| internal(format!("load metrics: {e}")))?;
    let Some(def) = defs.iter().find(|d| d.name == asked.metric) else {
        return Err(not_found(format!("metric {:?} not found", asked.metric)));
    };
    let window = window(asked.secs, now_ns());
    let sql = series_sql(def, &asked, window)?;
    let skip_reason = sidecar_skip_reason(&state);
    let key = series_key(&asked);
    let cache = CacheQuery {
        key: key.clone(),
        model: cache_filter(state.forecast_client().is_some() && skip_reason.is_none()),
        horizon: asked.horizon,
    };
    let (series, cached) = load(&state, sql, window, cache).await?;
    telemetry::cache_lookup(cached.is_some());
    let answer = match cached {
        Some(row) => Answer {
            forecast: forecast_from_row(&row),
            fallback_reason: skip_reason,
            cached: true,
        },
        None => fresh(&state, &asked, &key, &series).await?,
    };
    Ok(Json(body(
        &asked,
        def.description.as_deref(),
        window,
        &series,
        &answer,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE_NS: i64 = 60 * SECOND_NS;

    fn asked_for(metric: &str, target: Option<&str>, envs: &[&str]) -> Asked {
        Asked {
            metric: metric.into(),
            target: target.map(str::to_owned),
            envs: envs.iter().map(|e| (*e).to_string()).collect(),
            range: "24h".into(),
            secs: 86_400,
            horizon: 1,
        }
    }

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

    /// A poll every second in the one-second tier would otherwise fit and
    /// store a forecast per second.
    #[test]
    fn the_one_second_tier_moves_its_window_once_a_minute() {
        let now = 1_700_000_000 * SECOND_NS + 17 * SECOND_NS + 5;
        let hour = window(3_600, now);
        assert_eq!(hour.step_ns, SECOND_NS);
        assert_eq!(hour.to_ns % MINUTE_NS, 0);
        assert!(now - hour.to_ns < MINUTE_NS);
        assert_eq!(hour.to_ns - hour.from_ns, 3_600 * SECOND_NS);
        assert_eq!(window(3_600, now + 20 * SECOND_NS), hour, "same minute");
        assert_ne!(window(3_600, now + 60 * SECOND_NS), hour);
    }

    #[test]
    fn coarser_tiers_align_to_their_own_step() {
        let now = 1_700_000_000 * SECOND_NS + 5;
        let day = window(86_400, now);
        assert_eq!(day.step_ns, MINUTE_NS);
        assert_eq!(day.to_ns % MINUTE_NS, 0);
        let quarter = window(90 * 86_400, now);
        assert_eq!(quarter.step_ns, 3_600 * SECOND_NS);
        assert_eq!(quarter.to_ns % (3_600 * SECOND_NS), 0);
    }

    /// With `;` and `=` as separators, target `a;env=b` and env `b` were
    /// one key.
    #[test]
    fn series_keys_cannot_collide_through_separators() {
        let plain = series_key(&asked_for("m", Some("a"), &["b"]));
        assert_eq!(plain, r#"["m","a",["b"]]"#);
        let hostile = [
            asked_for("m", Some("a;env=b"), &[]),
            asked_for("m;target=a", None, &["b"]),
            asked_for("m", Some("a\",[\"b"), &[]),
            asked_for("m", Some("a"), &["b,c"]),
            asked_for("m", Some("a"), &["b", "c"]),
            asked_for("m", None, &["a", "b"]),
            asked_for("m", Some(""), &["b"]),
            asked_for("m", None, &["b"]),
        ];
        let mut keys: Vec<String> = hostile.iter().map(series_key).collect();
        keys.push(plain);
        let distinct: std::collections::BTreeSet<&String> = keys.iter().collect();
        assert_eq!(distinct.len(), keys.len(), "{keys:?}");
    }

    #[test]
    fn a_row_round_trips_the_degenerate_flag() {
        let series = RegularSeries {
            start_ns: 0,
            step_ns: SECOND_NS,
            values: vec![5.0; 12],
        };
        let forecast = tumult_forecast::ForecastProvider::forecast(
            &tumult_forecast::HoltWinters::new(),
            &tumult_forecast::ForecastRequest::new(&series.values, 2),
        )
        .unwrap();
        assert!(forecast.degenerate_band, "a constant context has no band");
        let row = row_from_forecast("k", &series, &forecast, 9);
        assert!(row.degenerate_band);
        assert_eq!(row.context_end_ns, series.end_ns());
        assert_eq!(forecast_from_row(&row), forecast);
    }
}
