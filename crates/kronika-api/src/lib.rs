//! `kronika-api` — the read-only JSON query API backing the kronika UI.
//!
//! Routes (all under `/api`, all read-only against the store):
//!
//! * `GET /api/overview?range=24h|7d|14d` — KPI cards (value, delta vs the
//!   previous equal window, sparkline), experiments per day, target-system
//!   leaderboard, fault breakdown.
//! * `GET /api/timeseries?metric=<name>&interval=5m|1h|1d&range=…` — any
//!   semantic metric from the metrics directory as a bucketed series.
//! * `GET /api/experiments?range=&outcome=&target=&fault=&q=` — experiment
//!   list, newest first (outcome joined from tumult's `experiment.completed`
//!   log attributes; root spans carry no outcome for real tumult data).
//! * `GET /api/experiments/{id}` — spans (waterfall), correlated logs and
//!   metric points for one experiment.
//! * `GET /api/dimensions` — distinct filter values (outcomes, targets,
//!   faults, experiment names).
//! * `GET /api/metrics` — semantic metrics available for `/api/timeseries`.
//! * `POST /api/ask` — natural-language → SQL → rows, guarded by
//!   `kronika_ai::sql_guard`; degrades to `{configured:false}` when no LLM
//!   is reachable.
//! * `GET /api/reports` / `GET /api/reports/{name}` — HTML digests written
//!   by the daemon's report scheduler.
//!
//! Every query runs on a fresh read-only connection inside `spawn_blocking`,
//! so the API coexists with the daemon's single writer and never touches the
//! write lock.

mod ask;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kronika_store::{Reader, Store};
use serde::Deserialize;
use serde_json::{json, Value};

/// Span-name predicate selecting one row per experiment run.
const ROOT: &str = "span_name = 'resilience.experiment'";

/// Shared handler state: where the store, metric definitions and rendered
/// reports live, plus the LLM client for `/api/ask`.
#[derive(Clone)]
pub struct ApiState {
    db_path: Arc<PathBuf>,
    metrics_dir: Arc<PathBuf>,
    reports_dir: Arc<PathBuf>,
    llm: Arc<dyn kronika_ai::Llm>,
}

impl ApiState {
    /// Full constructor (tests inject a stub LLM and a scratch reports dir).
    #[must_use]
    pub fn new(
        db_path: PathBuf,
        metrics_dir: PathBuf,
        reports_dir: PathBuf,
        llm: Arc<dyn kronika_ai::Llm>,
    ) -> Self {
        Self {
            db_path: Arc::new(db_path),
            metrics_dir: Arc::new(metrics_dir),
            reports_dir: Arc::new(reports_dir),
            llm,
        }
    }

    /// Daemon constructor: reports live in `<db dir>/reports`, LLM configured
    /// from `KRONIKA_LLM_*` env vars.
    #[must_use]
    pub fn from_env_parts(db_path: PathBuf, metrics_dir: PathBuf) -> Self {
        let reports_dir = db_path
            .parent()
            .map_or_else(|| PathBuf::from("reports"), |d| d.join("reports"));
        Self::new(
            db_path,
            metrics_dir,
            reports_dir,
            Arc::new(kronika_ai::OpenAiCompatClient::from_env()),
        )
    }

    /// Directory the report scheduler writes into and `/api/reports` reads.
    #[must_use]
    pub fn reports_dir(&self) -> &PathBuf {
        &self.reports_dir
    }
}

/// Build the API router. Merge into the daemon's HTTP server.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/overview", get(overview))
        .route("/api/timeseries", get(timeseries))
        .route("/api/experiments", get(experiments))
        .route("/api/experiments/{id}", get(experiment_detail))
        .route("/api/dimensions", get(dimensions))
        .route("/api/metrics", get(list_metrics))
        .route("/api/ask", post(ask::ask))
        .route("/api/reports", get(list_reports))
        .route("/api/reports/{name}", get(get_report))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// helpers

/// Current time as epoch nanoseconds.
fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

/// `24h` / `7d` / `14d` → seconds.
fn parse_range(range: &str) -> Option<i64> {
    match range {
        "24h" => Some(86_400),
        "7d" => Some(7 * 86_400),
        "14d" => Some(14 * 86_400),
        _ => None,
    }
}

/// `5m` / `1h` / `1d` → seconds.
fn parse_interval(interval: &str) -> Option<i64> {
    match interval {
        "5m" => Some(300),
        "1h" => Some(3_600),
        "1d" => Some(86_400),
        _ => None,
    }
}

/// Window `[from, to)` for `range` ending at now, and the previous equal
/// window before it.
fn windows(range: &str) -> Option<((i64, i64), (i64, i64))> {
    let secs = parse_range(range)?;
    let to = now_ns();
    let from = to - secs * 1_000_000_000;
    Some(((from, to), (from - secs * 1_000_000_000, from)))
}

/// Quote a user-supplied string as a SQL string literal.
fn sql_string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Quote a user-supplied substring for `ILIKE … ESCAPE '\'` (contains-match).
fn sql_contains(s: &str) -> String {
    let esc = s
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
        .replace('\'', "''");
    format!("'%{esc}%'")
}

/// Time-window predicate on `ts_ns`.
fn w(from: i64, to: i64) -> String {
    format!("ts_ns >= {from} AND ts_ns < {to}")
}

/// 500 JSON error response.
fn internal(msg: String) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": msg})),
    )
        .into_response()
}

/// Run `f` with a fresh read-only reader on a blocking thread; map any
/// failure to a 500 JSON error.
async fn with_reader<T>(
    db_path: &std::path::Path,
    f: impl FnOnce(&Reader) -> Result<T, String> + Send + 'static,
) -> Result<T, Response>
where
    T: Send + 'static,
{
    let db = db_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let store = Store::at(&db);
        let reader = store.read_only().map_err(|e| e.to_string())?;
        f(&reader)
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": format!("query task failed: {e}")})),
        )
            .into_response()
    })?
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({"error": e}))).into_response())
}

/// First row's `v` column as `f64` (`None` when no rows or NULL).
fn scalar(reader: &Reader, sql: &str) -> Result<Option<f64>, String> {
    let rows = reader.query_json_rows(sql).map_err(|e| e.to_string())?;
    Ok(rows
        .first()
        .and_then(|r| r.get("v"))
        .and_then(Value::as_f64))
}

/// `{ts, v}` rows as a JSON array (bucketed series).
fn series(reader: &Reader, sql: &str) -> Result<Vec<Value>, String> {
    let rows = reader.query_json_rows(sql).map_err(|e| e.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let ts = r.get("ts")?.as_i64()?;
            let v = r.get("v").and_then(Value::as_f64)?;
            Some(json!({"ts": ts, "v": v}))
        })
        .collect())
}

/// Divide two aligned bucketed series pointwise (missing numerator → 0,
/// missing/zero denominator → point dropped).
fn ratio_series(num: &[Value], den: &[Value]) -> Vec<Value> {
    let den: HashMap<i64, f64> = den
        .iter()
        .filter_map(|r| Some((r.get("ts")?.as_i64()?, r.get("v")?.as_f64()?)))
        .collect();
    let mut out: Vec<Value> = den
        .iter()
        .filter(|(_, d)| **d > 0.0)
        .map(|(ts, d)| {
            let n = num
                .iter()
                .find(|r| r.get("ts").and_then(Value::as_i64) == Some(*ts))
                .and_then(|r| r.get("v").and_then(Value::as_f64))
                .unwrap_or(0.0);
            json!({"ts": ts, "v": n / d})
        })
        .collect();
    out.sort_by_key(|r| r.get("ts").and_then(Value::as_i64).unwrap_or(0));
    out
}

// ---------------------------------------------------------------------------
// GET /api/overview

#[derive(Deserialize)]
struct OverviewParams {
    range: Option<String>,
}

/// SQL building blocks for one KPI card: single-value query per window, and
/// a bucketed sparkline query (either one series or a num/den pair).
struct Kpi {
    name: &'static str,
    label: &'static str,
    /// `"count"`, `"ratio"` or `"seconds"` — the UI formats accordingly.
    unit: &'static str,
    value: fn(i64, i64) -> String,
    spark_num: fn(i64, i64, i64) -> String,
    spark_den: Option<fn(i64, i64, i64) -> String>,
}

fn bucket_expr(bucket_s: i64) -> String {
    let ns = bucket_s * 1_000_000_000;
    format!("(ts_ns // {ns}) * {ns} // 1000000000")
}

fn sql_experiments_value(f: i64, t: i64) -> String {
    format!(
        "SELECT COUNT(*) AS v FROM spans WHERE {ROOT} AND {}",
        w(f, t)
    )
}

fn sql_experiments_spark(f: i64, t: i64, b: i64) -> String {
    format!(
        "SELECT {} AS ts, COUNT(*) AS v FROM spans WHERE {ROOT} AND {} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t)
    )
}

fn sql_pass_rate_value(f: i64, t: i64) -> String {
    format!(
        "SELECT SUM(CAST(value AS DOUBLE)) FILTER (WHERE outcome_status = 'success') \
         / NULLIF(SUM(CAST(value AS DOUBLE)), 0) AS v \
         FROM metric_sums WHERE metric_name = 'tumult.experiments.total' AND {}",
        w(f, t)
    )
}

fn sql_pass_num(f: i64, t: i64, b: i64) -> String {
    format!(
        "SELECT {} AS ts, SUM(CAST(value AS DOUBLE)) AS v FROM metric_sums \
         WHERE metric_name = 'tumult.experiments.total' AND outcome_status = 'success' AND {} \
         GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t)
    )
}

fn sql_pass_den(f: i64, t: i64, b: i64) -> String {
    format!(
        "SELECT {} AS ts, SUM(CAST(value AS DOUBLE)) AS v FROM metric_sums \
         WHERE metric_name = 'tumult.experiments.total' AND {} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t)
    )
}

fn sql_deviation_value(f: i64, t: i64) -> String {
    format!(
        "SELECT (SELECT COALESCE(SUM(CAST(value AS DOUBLE)), 0) FROM metric_sums \
         WHERE metric_name = 'tumult.hypothesis.deviations.total' AND {w}) \
         / NULLIF((SELECT COUNT(*) FROM spans WHERE {ROOT} AND {w}), 0) AS v",
        w = w(f, t)
    )
}

fn sql_deviation_num(f: i64, t: i64, b: i64) -> String {
    format!(
        "SELECT {} AS ts, COALESCE(SUM(CAST(value AS DOUBLE)), 0) AS v FROM metric_sums \
         WHERE metric_name = 'tumult.hypothesis.deviations.total' AND {} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t)
    )
}

fn sql_mttr_value(f: i64, t: i64) -> String {
    format!(
        "SELECT AVG(recovery_time_s) AS v FROM spans WHERE recovery_time_s IS NOT NULL AND {}",
        w(f, t)
    )
}

fn sql_mttr_spark(f: i64, t: i64, b: i64) -> String {
    format!(
        "SELECT {} AS ts, AVG(recovery_time_s) AS v FROM spans \
         WHERE recovery_time_s IS NOT NULL AND {} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t)
    )
}

fn sql_coverage_value(f: i64, t: i64) -> String {
    format!(
        "SELECT COUNT(DISTINCT target_system) AS v FROM spans WHERE target_system IS NOT NULL AND {}",
        w(f, t)
    )
}

fn sql_coverage_spark(f: i64, t: i64, b: i64) -> String {
    format!(
        "SELECT {} AS ts, COUNT(DISTINCT target_system) AS v FROM spans \
         WHERE target_system IS NOT NULL AND {} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t)
    )
}

const KPIS: &[Kpi] = &[
    Kpi {
        name: "experiments",
        label: "Experiments",
        unit: "count",
        value: sql_experiments_value,
        spark_num: sql_experiments_spark,
        spark_den: None,
    },
    Kpi {
        name: "pass_rate",
        label: "Hypothesis pass rate",
        unit: "ratio",
        value: sql_pass_rate_value,
        spark_num: sql_pass_num,
        spark_den: Some(sql_pass_den),
    },
    Kpi {
        name: "deviation_rate",
        label: "Deviation rate",
        unit: "ratio",
        value: sql_deviation_value,
        spark_num: sql_deviation_num,
        spark_den: Some(sql_experiments_spark),
    },
    Kpi {
        name: "mttr_s",
        label: "Mean time to recover",
        unit: "seconds",
        value: sql_mttr_value,
        spark_num: sql_mttr_spark,
        spark_den: None,
    },
    Kpi {
        name: "coverage",
        label: "Target coverage",
        unit: "count",
        value: sql_coverage_value,
        spark_num: sql_coverage_spark,
        spark_den: None,
    },
];

async fn overview(
    State(state): State<ApiState>,
    Query(params): Query<OverviewParams>,
) -> Result<Json<Value>, Response> {
    let range = params.range.unwrap_or_else(|| "24h".into());
    let Some((cur, prev)) = windows(&range) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("invalid range {range:?}; expected 24h|7d|14d")})),
        )
            .into_response());
    };
    // Short ranges get hourly sparkline buckets, longer ones daily.
    let bucket_s: i64 = if range == "24h" { 3_600 } else { 86_400 };

    let body = with_reader(&state.db_path, move |reader| {
        let mut kpis = Vec::new();
        for kpi in KPIS {
            let value = scalar(reader, &(kpi.value)(cur.0, cur.1))?;
            let prev_value = scalar(reader, &(kpi.value)(prev.0, prev.1))?;
            let delta = value.zip(prev_value).map(|(v, p)| v - p);
            let num = series(reader, &(kpi.spark_num)(cur.0, cur.1, bucket_s))?;
            let spark = match kpi.spark_den {
                Some(den_fn) => {
                    let den = series(reader, &den_fn(cur.0, cur.1, bucket_s))?;
                    ratio_series(&num, &den)
                }
                None => num,
            };
            kpis.push(json!({
                "name": kpi.name,
                "label": kpi.label,
                "unit": kpi.unit,
                "value": value,
                "delta": delta,
                "spark": spark,
            }));
        }

        let per_day = series(
            reader,
            &format!(
                "SELECT {} AS ts, COUNT(*) AS v FROM spans WHERE {ROOT} AND {} \
                 GROUP BY 1 ORDER BY 1",
                bucket_expr(86_400),
                w(cur.0, cur.1)
            ),
        )?;

        let targets = reader
            .query_json_rows(&format!(
                "SELECT s.target_system AS target, COUNT(*) AS experiments, \
                 COUNT(*) FILTER (WHERE l.log_attrs['status'] = 'Completed') \
                 / NULLIF(COUNT(*), 0) AS pass_rate \
                 FROM spans s LEFT JOIN logs l \
                   ON l.log_attrs['experiment_id'] = s.experiment_id \
                  AND l.body = 'experiment.completed' \
                 WHERE s.span_name = 'resilience.experiment' \
                   AND s.target_system IS NOT NULL \
                   AND s.ts_ns >= {} AND s.ts_ns < {} \
                 GROUP BY 1 ORDER BY experiments DESC LIMIT 10",
                cur.0, cur.1
            ))
            .map_err(|e| e.to_string())?;

        let faults = reader
            .query_json_rows(&format!(
                "SELECT fault_type, fault_subtype, COUNT(*) AS count FROM spans \
                 WHERE fault_type IS NOT NULL AND {} \
                 GROUP BY 1, 2 ORDER BY count DESC LIMIT 10",
                w(cur.0, cur.1)
            ))
            .map_err(|e| e.to_string())?;

        Ok(json!({
            "range": range,
            "from_ns": cur.0,
            "to_ns": cur.1,
            "kpis": kpis,
            "experiments_per_day": per_day,
            "targets": targets,
            "faults": faults,
        }))
    })
    .await?;
    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// GET /api/timeseries

#[derive(Deserialize)]
struct TimeseriesParams {
    metric: Option<String>,
    interval: Option<String>,
    range: Option<String>,
}

async fn timeseries(
    State(state): State<ApiState>,
    Query(params): Query<TimeseriesParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let Some(metric) = params.metric.filter(|m| !m.is_empty()) else {
        return Err(bad("missing query parameter: metric".into()));
    };
    let interval = params.interval.unwrap_or_else(|| "1h".into());
    let Some(bucket_s) = parse_interval(&interval) else {
        return Err(bad(format!(
            "invalid interval {interval:?}; expected 5m|1h|1d"
        )));
    };
    let range = params.range.unwrap_or_else(|| "24h".into());
    let Some((cur, _)) = windows(&range) else {
        return Err(bad(format!("invalid range {range:?}; expected 24h|7d|14d")));
    };

    let metrics_dir = state.metrics_dir.as_ref().clone();
    let defs = kronika_metrics::load_dir(&metrics_dir)
        .map_err(|e| internal(format!("load metrics: {e}")))?;
    let Some(def) = defs.iter().find(|d| d.name == metric) else {
        let available = defs
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("metric {metric:?} not found; available: {available}")})),
        )
            .into_response());
    };
    let sql =
        kronika_metrics::to_sql_bucketed(def, bucket_s * 1_000_000_000, &[], Some((cur.0, cur.1)))
            .map_err(|e| internal(e.to_string()))?;
    let description = def.description.clone();
    let body = with_reader(&state.db_path, move |reader| {
        let rows = reader.query_json_rows(&sql).map_err(|e| e.to_string())?;
        Ok(json!({
            "metric": metric,
            "description": description,
            "interval": interval,
            "range": range,
            "points": rows,
        }))
    })
    .await?;
    Ok(Json(body))
}

// ---------------------------------------------------------------------------
// GET /api/experiments + /api/experiments/{id}

#[derive(Deserialize)]
struct ExperimentParams {
    range: Option<String>,
    outcome: Option<String>,
    target: Option<String>,
    fault: Option<String>,
    q: Option<String>,
}

/// The root-span ↔ completed-log join used by the list and detail queries.
const EXPERIMENT_FROM: &str = "FROM spans s LEFT JOIN logs l \
     ON l.log_attrs['experiment_id'] = s.experiment_id \
    AND l.body = 'experiment.completed'";

const EXPERIMENT_COLS: &str = "s.experiment_id AS id, s.experiment_name AS name, \
     s.ts_ns AS started_ns, s.duration_ns, s.trace_id, \
     s.target_system, s.target_technology, s.target_environment, \
     l.log_attrs['status'] AS status, \
     l.log_attrs['deviations'] AS deviations, \
     l.log_attrs['duration_ms'] AS duration_ms";

async fn experiments(
    State(state): State<ApiState>,
    Query(params): Query<ExperimentParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let mut wheres = vec!["s.span_name = 'resilience.experiment'".to_string()];
    if let Some(range) = &params.range {
        let Some((cur, _)) = windows(range) else {
            return Err(bad(format!("invalid range {range:?}; expected 24h|7d|14d")));
        };
        wheres.push(format!("s.ts_ns >= {} AND s.ts_ns < {}", cur.0, cur.1));
    }
    if let Some(outcome) = &params.outcome {
        let lower = outcome.to_ascii_lowercase();
        match lower.as_str() {
            // tumult stores the status capitalised in log attributes.
            "completed" | "deviated" | "failed" => {
                let mut chars = lower.chars();
                let cap = chars.next().map_or_else(String::new, |f| {
                    f.to_uppercase().collect::<String>() + chars.as_str()
                });
                wheres.push(format!("l.log_attrs['status'] = {}", sql_string(&cap)));
            }
            "incomplete" => wheres.push("l.log_attrs['status'] IS NULL".into()),
            other => {
                return Err(bad(format!(
                    "invalid outcome {other:?}; expected completed|deviated|failed|incomplete"
                )))
            }
        }
    }
    if let Some(target) = &params.target {
        wheres.push(format!("s.target_system = {}", sql_string(target)));
    }
    if let Some(fault) = &params.fault {
        // Child spans (actions/probes) carry the fault but not experiment_id —
        // they correlate with the run through trace_id.
        wheres.push(format!(
            "EXISTS (SELECT 1 FROM spans c WHERE c.trace_id = s.trace_id \
             AND c.fault_type = {})",
            sql_string(fault)
        ));
    }
    if let Some(q) = &params.q {
        if q.chars().count() > 200 {
            return Err(bad("q too long (max 200 chars)".into()));
        }
        let like = sql_contains(q);
        wheres.push(format!(
            "(s.experiment_name ILIKE {like} ESCAPE '\\' \
             OR s.experiment_id ILIKE {like} ESCAPE '\\')"
        ));
    }

    let sql = format!(
        "SELECT {EXPERIMENT_COLS}, \
         (SELECT string_agg(DISTINCT c.fault_type, ',') FROM spans c \
          WHERE c.trace_id = s.trace_id AND c.fault_type IS NOT NULL) AS faults \
         {EXPERIMENT_FROM} WHERE {} ORDER BY s.ts_ns DESC LIMIT 500",
        wheres.join(" AND ")
    );
    let rows = with_reader(&state.db_path, move |reader| {
        reader.query_json_rows(&sql).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "experiments": rows})))
}

async fn experiment_detail(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    if id.chars().count() > 100 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "experiment id too long"})),
        )
            .into_response());
    }
    let body = with_reader(&state.db_path, move |reader| {
        let id_sql = sql_string(&id);
        let info = reader
            .query_json_rows(&format!(
                "SELECT {EXPERIMENT_COLS} {EXPERIMENT_FROM} \
                 WHERE s.span_name = 'resilience.experiment' AND s.experiment_id = {id_sql}"
            ))
            .map_err(|e| e.to_string())?;
        let Some(info) = info.into_iter().next() else {
            return Ok(None);
        };

        // Tumult sets experiment_id only on the root span; the rest of the
        // run's span tree (probes, actions, rollbacks) shares its trace_id.
        let spans = reader
            .query_json_rows(&format!(
                "SELECT ts_ns, trace_id, span_id, parent_span_id, span_name, span_kind, \
                 duration_ns, status_code, status_message, service_name, \
                 fault_type, fault_subtype, span_attrs, events \
                 FROM spans \
                 WHERE experiment_id = {id_sql} \
                    OR trace_id IN (SELECT trace_id FROM spans WHERE experiment_id = {id_sql}) \
                 ORDER BY ts_ns LIMIT 2000"
            ))
            .map_err(|e| e.to_string())?;

        let trace_ids: Vec<String> = spans
            .iter()
            .filter_map(|s| s.get("trace_id").and_then(Value::as_str))
            .map(sql_string)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut log_where = format!("log_attrs['experiment_id'] = {id_sql}");
        if !trace_ids.is_empty() {
            log_where.push_str(&format!(" OR trace_id IN ({})", trace_ids.join(", ")));
        }
        let logs = reader
            .query_json_rows(&format!(
                "SELECT ts_ns, severity_text, body, trace_id, span_id, log_attrs \
                 FROM logs WHERE {log_where} ORDER BY ts_ns LIMIT 1000"
            ))
            .map_err(|e| e.to_string())?;

        let metrics = match info.get("name").and_then(Value::as_str) {
            Some(name) => {
                let name = sql_string(name);
                reader
                    .query_json_rows(&format!(
                        "SELECT 'sum' AS kind, ts_ns, metric_name, value, outcome_status, plugin_name \
                         FROM metric_sums WHERE experiment_name = {name} \
                         UNION ALL \
                         SELECT 'gauge', ts_ns, metric_name, value, outcome_status, plugin_name \
                         FROM metric_gauges WHERE experiment_name = {name} \
                         ORDER BY 2 LIMIT 1000"
                    ))
                    .map_err(|e| e.to_string())?
            }
            None => Vec::new(),
        };

        Ok(Some(json!({
            "experiment": info,
            "spans": spans,
            "logs": logs,
            "metrics": metrics,
        })))
    })
    .await?;
    match body {
        Some(body) => Ok(Json(body)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "experiment not found"})),
        )
            .into_response()),
    }
}

// ---------------------------------------------------------------------------
// GET /api/dimensions + /api/metrics

async fn dimensions(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let body = with_reader(&state.db_path, move |reader| {
        let distinct = |sql: &str| -> Result<Vec<Value>, String> {
            let rows = reader.query_json_rows(sql).map_err(|e| e.to_string())?;
            Ok(rows
                .into_iter()
                .filter_map(|r| r.get("v").cloned())
                .collect())
        };
        Ok(json!({
            "outcomes": distinct(
                "SELECT DISTINCT log_attrs['status'] AS v FROM logs \
                 WHERE body = 'experiment.completed' AND log_attrs['status'] IS NOT NULL ORDER BY 1")?,
            "targets": distinct(
                "SELECT DISTINCT target_system AS v FROM spans \
                 WHERE target_system IS NOT NULL ORDER BY 1")?,
            "faults": distinct(
                "SELECT DISTINCT fault_type AS v FROM spans \
                 WHERE fault_type IS NOT NULL ORDER BY 1")?,
            "experiments": distinct(
                "SELECT DISTINCT experiment_name AS v FROM spans \
                 WHERE span_name = 'resilience.experiment' \
                   AND experiment_name IS NOT NULL ORDER BY 1")?,
        }))
    })
    .await?;
    Ok(Json(body))
}

async fn list_metrics(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let defs = kronika_metrics::load_dir(state.metrics_dir.as_ref())
        .map_err(|e| internal(format!("load metrics: {e}")))?;
    let metrics: Vec<Value> = defs
        .iter()
        .map(|d| json!({"name": d.name, "description": d.description}))
        .collect();
    Ok(Json(json!({"metrics": metrics})))
}

// ---------------------------------------------------------------------------
// GET /api/reports + /api/reports/{name}

async fn list_reports(State(state): State<ApiState>) -> Json<Value> {
    let mut reports = Vec::new();
    if let Ok(entries) = std::fs::read_dir(state.reports_dir.as_ref()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "html") {
                let meta = entry.metadata().ok();
                let modified_s = meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_secs() as i64);
                reports.push(json!({
                    "name": entry.file_name().to_string_lossy(),
                    "bytes": meta.map_or(0, |m| m.len()),
                    "modified_s": modified_s,
                }));
            }
        }
    }
    // Timestamp-prefixed names sort newest first, lexicographically.
    reports.sort_by_key(|r| r.get("name").and_then(Value::as_str).map(str::to_owned));
    reports.reverse();
    Json(json!({"reports": reports}))
}

async fn get_report(State(state): State<ApiState>, Path(name): Path<String>) -> Response {
    // No path traversal: a report name is a flat file name only.
    if name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || !name.ends_with(".html")
        || name.len() > 200
    {
        return (StatusCode::BAD_REQUEST, "invalid report name").into_response();
    }
    match std::fs::read_to_string(state.reports_dir.join(&name)) {
        Ok(html) => Html(html).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "report not found").into_response(),
    }
}
