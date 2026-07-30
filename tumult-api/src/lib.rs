// Imported from kronika (Apache-2.0, same author). Pedantic lints are
// scoped to tumult-native crates: this crate predates the pedantic gate and
// carries intentional patterns it flags (timestamp/score casts, f64
// comparisons). CI still applies -D warnings to it.
#![allow(clippy::pedantic)]

//! `tumult-api` — the read-only JSON query API backing the kronika UI.
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
//! * `GET /api/logs?range=&severity=&service=&q=&limit=` — raw log rows,
//!   newest first (severity is a case-insensitive exact match, `q` a
//!   contains-match on the body).
//! * `GET /api/logs/volume?range=&interval=&severity=&service=&q=` — log
//!   volume bucketed per severity for the explorer's stacked bar.
//! * `GET /api/traces?range=&service=&min_duration_ms=&outcome=` — traces
//!   grouped from spans (root name/service, span/error counts, experiment
//!   outcome where the trace is an experiment run).
//! * `GET /api/traces/durations?range=` — root-span durations as scatter
//!   points plus p50/p95/p99 percentiles.
//! * `GET /api/traces/{id}` — every span and log of one trace.
//! * `GET /api/metrics/catalog` — raw metric names across sums/gauges/
//!   histograms, with the attribute keys seen on their points.
//! * `GET /api/metrics/query?name=&group_by=&range=&interval=` — bucketed
//!   series for one raw metric (sums → SUM, gauges → AVG, histograms →
//!   avg plus an interpolated p95), optionally split by an attribute key.
//! * `GET /api/topology?range=` — service/target call graph: nodes from
//!   `service_name` and tumult's `resilience.target.name` span attribute,
//!   edges from parent→child span joins and service→target calls.
//! * `POST /api/ask` — natural-language → SQL → rows, guarded by
//!   `tumult_intelligence::sql_guard`; degrades to `{configured:false}` when no LLM
//!   is reachable.
//! * `GET /api/reports` / `GET /api/reports/{name}` — HTML digests written
//!   by the daemon's report scheduler; `POST /api/reports/generate` renders
//!   one metric digest on demand into the same directory.
//! * `POST /api/import/journal {journal, experiment?}` — daemon-first
//!   journal ingest for the CLI (`TUMULT_DAEMON_URL`): rides the
//!   single-writer channel into the analytics tables, idempotent on
//!   `experiment_id`.
//! * `POST /api/runs/validate {toon, vars?}` — the CLI's full
//!   parse/resolve/validate pipeline as a service; registers the definition
//!   (content-hash dedup) and returns its `registry_id`.
//! * `POST /api/runs/dry-run {registry_id, vars?}` — the resolved execution
//!   plan (hypothesis probes, method steps in order, guards, rollbacks)
//!   with nothing executed.
//! * `POST /api/runs {registry_id, vars?}` — enqueue onto the daemon's
//!   bounded run queue: 202 + `run_id`, 429 on overload (never silently
//!   queued). `POST /api/runs/{id}/stop` e-stops a run (mid-method cancel
//!   with rollbacks, or cancel-before-start when still queued).
//! * `GET /api/runs?state=&limit=` / `GET /api/runs/{id}` — run list and
//!   one run with its audit trail.
//! * `GET /api/scores?range=` — Gremlin-style resilience scorecard
//!   (freshness-decayed per-experiment scores, target and portfolio rollup).
//! * `POST /api/reports/v2/generate {type,period?,experiment_id?,framework?}`
//!   — build a compliance-grade report (R1 executive digest, R3 game-day,
//!   R2 evidence pack) as PDF + print-HTML + JSON meta under
//!   `reports/v2/`; `GET /api/reports/v2` lists metas and
//!   `GET /api/reports/v2/{id}/pdf|html` serves the artifacts.
//!
//! Every query runs on a fresh read-only connection inside `spawn_blocking`,
//! so the API coexists with the daemon's single writer and never touches the
//! write lock.

mod ask;
pub mod import;
pub mod lake;
pub mod manual;
pub mod runs;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_lake::{Reader, Store};

/// Span-name predicate selecting one row per experiment run.
const ROOT: &str = "span_name = 'resilience.experiment'";

/// Shared handler state: where the store, metric definitions and rendered
/// reports live, plus the LLM client for `/api/ask`, the org tree for
/// `/api/scores/tree` and R1's "By domain", the ingest handle that
/// carries manual-evidence mutations onto the daemon's single writer, and
/// the bounded run queue behind `/api/runs*`.
#[derive(Clone)]
pub struct ApiState {
    db_path: Arc<PathBuf>,
    metrics_dir: Arc<PathBuf>,
    reports_dir: Arc<PathBuf>,
    llm: Arc<dyn tumult_intelligence::llm::Llm>,
    org: Arc<tumult_compliance::OrgTree>,
    ingest: Option<tumult_ingest::IngestWriter>,
    runs: Option<tumult_ingest::RunQueue>,
}

impl ApiState {
    /// Full constructor (tests inject a stub LLM and a scratch reports dir).
    #[must_use]
    pub fn new(
        db_path: PathBuf,
        metrics_dir: PathBuf,
        reports_dir: PathBuf,
        llm: Arc<dyn tumult_intelligence::llm::Llm>,
        org: tumult_compliance::OrgTree,
        ingest: Option<tumult_ingest::IngestWriter>,
        runs: Option<tumult_ingest::RunQueue>,
    ) -> Self {
        Self {
            db_path: Arc::new(db_path),
            metrics_dir: Arc::new(metrics_dir),
            reports_dir: Arc::new(reports_dir),
            llm,
            org: Arc::new(org),
            ingest,
            runs,
        }
    }

    /// Daemon constructor: reports live in `<db dir>/reports`, LLM configured
    /// from `KRONIKA_LLM_*` env vars. The org tree loads from
    /// `KRONIKA_ORG_FILE`, defaulting to `<db dir>/org.yaml`; a missing file
    /// means an empty tree (everything rolls up under `(unassigned)`) and an
    /// invalid file logs a warning and falls back to empty.
    #[must_use]
    pub fn from_env_parts(
        db_path: PathBuf,
        metrics_dir: PathBuf,
        ingest: Option<tumult_ingest::IngestWriter>,
        runs: Option<tumult_ingest::RunQueue>,
    ) -> Self {
        let reports_dir = db_path
            .parent()
            .map_or_else(|| PathBuf::from("reports"), |d| d.join("reports"));
        let org_path = std::env::var_os("KRONIKA_ORG_FILE")
            .map(PathBuf::from)
            .or_else(|| db_path.parent().map(|d| d.join("org.yaml")));
        let org = org_path
            .filter(|p| p.exists())
            .map_or_else(tumult_compliance::OrgTree::empty, |p| {
                tumult_compliance::OrgTree::load(&p).unwrap_or_else(|e| {
                    tracing::warn!(path = %p.display(), error = %e, "invalid org file; using empty tree");
                    tumult_compliance::OrgTree::empty()
                })
            });
        Self::new(
            db_path,
            metrics_dir,
            reports_dir,
            Arc::new(tumult_intelligence::llm::OpenAiCompatClient::from_env()),
            org,
            ingest,
            runs,
        )
    }

    /// Directory the report scheduler writes into and `/api/reports` reads.
    #[must_use]
    pub fn reports_dir(&self) -> &PathBuf {
        &self.reports_dir
    }

    /// The ingest handle carrying manual-evidence writes (daemon only).
    #[must_use]
    pub fn ingest_handle(&self) -> Option<&tumult_ingest::IngestWriter> {
        self.ingest.as_ref()
    }

    /// The bounded run queue behind `/api/runs*` (daemon only).
    #[must_use]
    pub fn runs_handle(&self) -> Option<&tumult_ingest::RunQueue> {
        self.runs.as_ref()
    }
}

/// Build the API router. Merge into the daemon's HTTP server.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/api/overview", get(overview))
        .route("/api/timeseries", get(timeseries))
        .route("/api/experiments", get(experiments))
        .route("/api/experiments/windows", get(experiment_windows))
        .route("/api/experiments/{id}", get(experiment_detail))
        .route("/api/dimensions", get(dimensions))
        .route("/api/metrics", get(list_metrics))
        .route("/api/logs", get(logs))
        .route("/api/logs/volume", get(logs_volume))
        .route("/api/traces", get(traces))
        .route("/api/traces/durations", get(trace_durations))
        .route("/api/traces/{id}", get(trace_detail))
        .route("/api/metrics/catalog", get(metrics_catalog))
        .route("/api/metrics/query", get(metrics_query))
        .route("/api/topology", get(topology))
        .route("/api/ask", post(ask::ask))
        .route("/api/scores/tree", get(scores_tree))
        .route(
            "/api/manual/experiments",
            get(manual::list).post(manual::create),
        )
        .route(
            "/api/manual/experiments/{id}",
            get(manual::detail).put(manual::update),
        )
        .route("/api/manual/experiments/{id}/submit", post(manual::submit))
        .route("/api/manual/experiments/{id}/verify", post(manual::verify))
        .route("/api/manual/experiments/{id}/reject", post(manual::reject))
        .route(
            "/api/manual/experiments/{id}/attachments",
            post(manual::attach),
        )
        .route("/api/manual/import", post(manual::import))
        .route("/api/import/journal", post(import::import_journal))
        .route("/api/runs/validate", post(runs::validate))
        .route("/api/runs/dry-run", post(runs::dry_run))
        .route("/api/runs", get(runs::list).post(runs::create))
        .route("/api/runs/{id}", get(runs::detail))
        .route("/api/runs/{id}/stop", post(runs::stop))
        .route("/api/lake/status", get(lake::status))
        .route("/api/lake/export", post(lake::export_now))
        .route("/api/reports", get(list_reports))
        .route("/api/reports/generate", post(generate_report))
        .route("/api/reports/v2", get(list_reports_v2))
        .route("/api/reports/v2/generate", post(generate_report_v2))
        .route("/api/reports/v2/{id}/pdf", get(get_report_v2_pdf))
        .route("/api/reports/v2/{id}/html", get(get_report_v2_html))
        .route("/api/scores", get(scores))
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

/// Quote a user-supplied string as an `ILIKE` pattern with no wildcards —
/// a case-insensitive exact match (`%`/`_` escaped, use with `ESCAPE '\'`).
fn sql_ieq(s: &str) -> String {
    let esc = s
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
        .replace('\'', "''");
    format!("'{esc}'")
}

/// Time-window predicate on `ts_ns`.
fn w(from: i64, to: i64) -> String {
    format!("ts_ns >= {from} AND ts_ns < {to}")
}

/// Parse a click-to-filter `k=v` parameter into (key, value); the key must
/// be non-empty (the value may be, to filter for empty attrs).
fn attr_kv(s: &str) -> Option<(&str, &str)> {
    let (k, v) = s.split_once('=')?;
    if k.is_empty() {
        None
    } else {
        Some((k, v))
    }
}

/// Click-to-filter predicates on the attribute maps, for the logs list
/// (`log_attrs` / `resource_attrs` columns) and — via `span_alias` — for the
/// traces list (`EXISTS` over any span of the trace). `attr` keeps only
/// rows where the key has exactly the value in either map; `attr_not`
/// excludes them (NULL-safe: rows lacking the key survive `attr_not`).
fn attr_wheres(
    attr: Option<&str>,
    attr_not: Option<&str>,
    key_cols: (&str, &str),
) -> Result<Vec<String>, String> {
    let (a, b) = key_cols;
    let mut wheres = Vec::new();
    if let Some(raw) = attr.filter(|s| !s.is_empty()) {
        let Some((k, v)) = attr_kv(raw) else {
            return Err(format!("invalid attr {raw:?}; expected k=v"));
        };
        wheres.push(format!(
            "({a}[{}] = {v} OR {b}[{}] = {v})",
            sql_string(k),
            sql_string(k),
            v = sql_string(v)
        ));
    }
    if let Some(raw) = attr_not.filter(|s| !s.is_empty()) {
        let Some((k, v)) = attr_kv(raw) else {
            return Err(format!("invalid attr_not {raw:?}; expected k=v"));
        };
        wheres.push(format!(
            "NOT (COALESCE({a}[{}] = {v}, false) OR COALESCE({b}[{}] = {v}, false))",
            sql_string(k),
            sql_string(k),
            v = sql_string(v)
        ));
    }
    Ok(wheres)
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
    let defs = tumult_metrics::load_dir(&metrics_dir)
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
        tumult_metrics::to_sql_bucketed(def, bucket_s * 1_000_000_000, &[], Some((cur.0, cur.1)))
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
    origin: Option<String>,
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
    let origin = params.origin.as_deref();
    if let Some(o) = origin {
        if o != "automated" && o != "manual" {
            return Err(bad(format!(
                "invalid origin {o:?}; expected automated|manual"
            )));
        }
    }
    // Manual records join the view unless explicitly excluded; they have no
    // tumult outcome/fault taxonomy, so those two filters drop the manual
    // branch entirely (documented behaviour).
    let include_manual =
        origin != Some("automated") && params.outcome.is_none() && params.fault.is_none();
    let include_spans = origin != Some("manual");

    let spans_sql = format!(
        "SELECT {EXPERIMENT_COLS}, \
         (SELECT string_agg(DISTINCT c.fault_type, ',') FROM spans c \
          WHERE c.trace_id = s.trace_id AND c.fault_type IS NOT NULL) AS faults, \
         'automated' AS origin, NULL AS review_status \
         {EXPERIMENT_FROM} WHERE {}",
        wheres.join(" AND ")
    );

    let sql = if !include_manual {
        format!("{spans_sql} ORDER BY s.ts_ns DESC LIMIT 500")
    } else {
        // The manual branch mirrors the span columns; range/target/q filters
        // map onto the manual columns.
        let mut mwheres = vec!["1=1".to_string()];
        if let Some(range) = &params.range {
            let Some((cur, _)) = windows(range) else {
                return Err(bad(format!("invalid range {range:?}; expected 24h|7d|14d")));
            };
            mwheres.push(format!(
                "m.executed_at_ns >= {} AND m.executed_at_ns < {}",
                cur.0, cur.1
            ));
        }
        if let Some(target) = &params.target {
            mwheres.push(format!("m.target_system = {}", sql_string(target)));
        }
        if let Some(q) = &params.q {
            let like = sql_contains(q);
            mwheres.push(format!(
                "(m.experiment_name ILIKE {like} ESCAPE '\\' \
                 OR m.id ILIKE {like} ESCAPE '\\')"
            ));
        }
        let manual_sql = format!(
            "SELECT m.id, m.experiment_name AS name, m.executed_at_ns AS started_ns, \
             CAST(m.duration_s * 1000000000 AS BIGINT) AS duration_ns, \
             NULL AS trace_id, m.target_system, NULL AS target_technology, \
             m.target_environment, m.outcome_status AS status, \
             NULL AS deviations, NULL AS duration_ms, NULL AS faults, \
             'manual' AS origin, m.status AS review_status \
             FROM manual_experiments m WHERE {}",
            mwheres.join(" AND ")
        );
        if include_spans {
            format!(
                "SELECT * FROM ({spans_sql} UNION ALL {manual_sql}) \
                 ORDER BY started_ns DESC LIMIT 500"
            )
        } else {
            format!("{manual_sql} ORDER BY started_ns DESC LIMIT 500")
        }
    };
    let rows = with_reader(&state.db_path, move |reader| {
        reader.query_json_rows(&sql).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "experiments": rows})))
}

#[derive(Deserialize)]
struct ExperimentWindowsParams {
    from: Option<i64>,
    to: Option<i64>,
}

/// `GET /api/experiments/windows?from=&to=` — experiment runs overlapping
/// the `[from, to)` window (epoch ns), for chart overlays. Cheaper than the
/// list endpoint: one row per run from the `experiment_runs` rollup view.
async fn experiment_windows(
    State(state): State<ApiState>,
    Query(params): Query<ExperimentWindowsParams>,
) -> Result<Json<Value>, Response> {
    let (Some(from), Some(to)) = (params.from, params.to) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "missing query parameters: from and to (epoch ns)"})),
        )
            .into_response());
    };
    if from >= to {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "from must be before to"})),
        )
            .into_response());
    }
    let sql = format!(
        "SELECT experiment_id AS id, experiment_name AS name, \
         started_at_ns AS start_ns, ended_at_ns AS end_ns, outcome_status AS outcome \
         FROM experiment_runs \
         WHERE started_at_ns < {to} AND ended_at_ns > {from} \
         ORDER BY started_at_ns LIMIT 200"
    );
    let rows = with_reader(&state.db_path, move |reader| {
        reader.query_json_rows(&sql).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "runs": rows})))
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
    let defs = tumult_metrics::load_dir(state.metrics_dir.as_ref())
        .map_err(|e| internal(format!("load metrics: {e}")))?;
    let metrics: Vec<Value> = defs
        .iter()
        .map(|d| json!({"name": d.name, "description": d.description}))
        .collect();
    Ok(Json(json!({"metrics": metrics})))
}

// ---------------------------------------------------------------------------
// GET /api/logs + /api/logs/volume

#[derive(Deserialize)]
struct LogsParams {
    range: Option<String>,
    severity: Option<String>,
    service: Option<String>,
    q: Option<String>,
    attr: Option<String>,
    attr_not: Option<String>,
    limit: Option<u32>,
}

#[derive(Deserialize)]
struct LogsVolumeParams {
    range: Option<String>,
    interval: Option<String>,
    severity: Option<String>,
    service: Option<String>,
    q: Option<String>,
    attr: Option<String>,
    attr_not: Option<String>,
}

/// WHERE clauses shared by the logs list and volume endpoints (range always
/// applies, so the list is never empty).
#[allow(clippy::too_many_arguments)]
fn log_wheres(
    range: Option<&str>,
    severity: Option<&str>,
    service: Option<&str>,
    q: Option<&str>,
    attr: Option<&str>,
    attr_not: Option<&str>,
) -> Result<Vec<String>, String> {
    let range = range.unwrap_or("24h");
    let Some((cur, _)) = windows(range) else {
        return Err(format!("invalid range {range:?}; expected 24h|7d|14d"));
    };
    let mut wheres = vec![w(cur.0, cur.1)];
    if let Some(sev) = severity.filter(|s| !s.is_empty()) {
        // Severity texts are short tokens (INFO/WARN/ERROR…): case-insensitive
        // exact match, not a contains search.
        wheres.push(format!("severity_text ILIKE {} ESCAPE '\\'", sql_ieq(sev)));
    }
    if let Some(svc) = service.filter(|s| !s.is_empty()) {
        wheres.push(format!("service_name = {}", sql_string(svc)));
    }
    if let Some(q) = q.filter(|s| !s.is_empty()) {
        if q.chars().count() > 200 {
            return Err("q too long (max 200 chars)".into());
        }
        wheres.push(format!("body ILIKE {} ESCAPE '\\'", sql_contains(q)));
    }
    wheres.extend(attr_wheres(
        attr,
        attr_not,
        ("log_attrs", "resource_attrs"),
    )?);
    Ok(wheres)
}

async fn logs(
    State(state): State<ApiState>,
    Query(params): Query<LogsParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let wheres = log_wheres(
        params.range.as_deref(),
        params.severity.as_deref(),
        params.service.as_deref(),
        params.q.as_deref(),
        params.attr.as_deref(),
        params.attr_not.as_deref(),
    )
    .map_err(bad)?;
    let limit = params.limit.unwrap_or(200).clamp(1, 1000);
    let sql = format!(
        "SELECT ts_ns, severity_text, body, trace_id, span_id, service_name, \
         log_attrs['experiment_id'] AS experiment_id, log_attrs, resource_attrs \
         FROM logs WHERE {} ORDER BY ts_ns DESC LIMIT {limit}",
        wheres.join(" AND ")
    );
    let rows = with_reader(&state.db_path, move |reader| {
        reader.query_json_rows(&sql).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "logs": rows})))
}

async fn logs_volume(
    State(state): State<ApiState>,
    Query(params): Query<LogsVolumeParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let interval = params.interval.unwrap_or_else(|| "1h".into());
    let Some(bucket_s) = parse_interval(&interval) else {
        return Err(bad(format!(
            "invalid interval {interval:?}; expected 5m|1h|1d"
        )));
    };
    let wheres = log_wheres(
        params.range.as_deref(),
        params.severity.as_deref(),
        params.service.as_deref(),
        params.q.as_deref(),
        params.attr.as_deref(),
        params.attr_not.as_deref(),
    )
    .map_err(bad)?;
    // One row per (bucket, severity); the UI pivots into stacked series.
    let sql = format!(
        "SELECT {} AS ts, COALESCE(severity_text, 'UNKNOWN') AS severity, COUNT(*) AS count \
         FROM logs WHERE {} GROUP BY 1, 2 ORDER BY 1",
        bucket_expr(bucket_s),
        wheres.join(" AND ")
    );
    let rows = with_reader(&state.db_path, move |reader| {
        reader.query_json_rows(&sql).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({
        "interval": interval,
        "bucket_s": bucket_s,
        "rows": rows,
    })))
}

// ---------------------------------------------------------------------------
// GET /api/traces + /api/traces/durations + /api/traces/{id}

#[derive(Deserialize)]
struct TracesParams {
    range: Option<String>,
    service: Option<String>,
    min_duration_ms: Option<f64>,
    outcome: Option<String>,
    attr: Option<String>,
    attr_not: Option<String>,
}

/// Reusable outcome predicate on the joined `experiment.completed` status
/// (`l` must be the logs alias of that join); `None` passes everything.
fn outcome_where(outcome: Option<&str>) -> Result<Option<String>, String> {
    let Some(outcome) = outcome.filter(|o| !o.is_empty()) else {
        return Ok(None);
    };
    let lower = outcome.to_ascii_lowercase();
    match lower.as_str() {
        // tumult stores the status capitalised in log attributes.
        "completed" | "deviated" | "failed" => {
            let mut chars = lower.chars();
            let cap = chars.next().map_or_else(String::new, |f| {
                f.to_uppercase().collect::<String>() + chars.as_str()
            });
            Ok(Some(format!(
                "l.log_attrs['status'] = {}",
                sql_string(&cap)
            )))
        }
        "incomplete" => Ok(Some("l.log_attrs['status'] IS NULL".into())),
        other => Err(format!(
            "invalid outcome {other:?}; expected completed|deviated|failed|incomplete"
        )),
    }
}

async fn traces(
    State(state): State<ApiState>,
    Query(params): Query<TracesParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();

    // Span-level window (traces are grouped only from spans inside it).
    let mut span_wheres = Vec::new();
    if let Some(range) = &params.range {
        let Some((cur, _)) = windows(range) else {
            return Err(bad(format!("invalid range {range:?}; expected 24h|7d|14d")));
        };
        span_wheres.push(w(cur.0, cur.1));
    }
    let span_where = if span_wheres.is_empty() {
        "TRUE".to_string()
    } else {
        span_wheres.join(" AND ")
    };

    // Trace-level filters (they must not shrink the span set being grouped).
    let mut trace_wheres = Vec::new();
    if let Some(service) = params.service.as_deref().filter(|s| !s.is_empty()) {
        trace_wheres.push(format!(
            "EXISTS (SELECT 1 FROM spans sx WHERE sx.trace_id = t.trace_id \
             AND sx.service_name = {})",
            sql_string(service)
        ));
    }
    if let Some(min_ms) = params.min_duration_ms {
        if !(min_ms.is_finite() && min_ms >= 0.0) {
            return Err(bad("min_duration_ms must be a non-negative number".into()));
        }
        trace_wheres.push(format!("t.duration_ns >= {}", (min_ms * 1e6) as i64));
    }
    if let Some(ow) = outcome_where(params.outcome.as_deref()).map_err(bad)? {
        trace_wheres.push(ow);
    }
    // Click-to-filter on span attributes (any span of the trace).
    for (raw, negate) in [
        (params.attr.as_deref(), false),
        (params.attr_not.as_deref(), true),
    ] {
        let Some(raw) = raw.filter(|s| !s.is_empty()) else {
            continue;
        };
        let Some((k, v)) = attr_kv(raw) else {
            return Err(bad(format!("invalid attr filter {raw:?}; expected k=v")));
        };
        let not = if negate { "NOT " } else { "" };
        trace_wheres.push(format!(
            "{not}EXISTS (SELECT 1 FROM spans sx WHERE sx.trace_id = t.trace_id \
             AND (sx.span_attrs[{k}] = {v} OR sx.resource_attrs[{k}] = {v}))",
            k = sql_string(k),
            v = sql_string(v)
        ));
    }
    let trace_where = if trace_wheres.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", trace_wheres.join(" AND "))
    };

    // Group spans into traces. The display root is the (earliest) span with
    // no parent; traces without one fall back to the earliest span. Only
    // experiment runs have an outcome, joined through the completed log.
    let sql = format!(
        "SELECT t.trace_id, t.started_ns, t.duration_ns, t.span_count, t.error_count, \
         COALESCE(r.span_name, t.first_span) AS root_name, \
         COALESCE(r.service_name, t.first_service) AS service_name, \
         t.experiment_id, t.experiment_name, \
         l.log_attrs['status'] AS status \
         FROM ( \
           SELECT trace_id, MIN(ts_ns) AS started_ns, \
                  MAX(ts_ns + duration_ns) - MIN(ts_ns) AS duration_ns, \
                  COUNT(*) AS span_count, \
                  COUNT(*) FILTER (WHERE status_code = 'Error') AS error_count, \
                  MAX(experiment_id) AS experiment_id, \
                  MAX(experiment_name) AS experiment_name, \
                  arg_min(span_name, ts_ns) AS first_span, \
                  arg_min(service_name, ts_ns) AS first_service \
           FROM spans WHERE {span_where} GROUP BY trace_id \
         ) t \
         LEFT JOIN ( \
           SELECT trace_id, span_name, service_name, \
                  ROW_NUMBER() OVER (PARTITION BY trace_id ORDER BY ts_ns) AS rn \
           FROM spans WHERE parent_span_id IS NULL \
         ) r ON r.trace_id = t.trace_id AND r.rn = 1 \
         LEFT JOIN logs l ON l.log_attrs['experiment_id'] = t.experiment_id \
            AND l.body = 'experiment.completed' \
         {trace_where} ORDER BY t.started_ns DESC LIMIT 200"
    );
    let rows = with_reader(&state.db_path, move |reader| {
        reader.query_json_rows(&sql).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "traces": rows})))
}

#[derive(Deserialize)]
struct TraceDurationsParams {
    range: Option<String>,
}

async fn trace_durations(
    State(state): State<ApiState>,
    Query(params): Query<TraceDurationsParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let mut span_where = "parent_span_id IS NULL".to_string();
    if let Some(range) = &params.range {
        let Some((cur, _)) = windows(range) else {
            return Err(bad(format!("invalid range {range:?}; expected 24h|7d|14d")));
        };
        span_where.push_str(&format!(" AND {}", w(cur.0, cur.1)));
    }
    let body = with_reader(&state.db_path, move |reader| {
        let points = reader
            .query_json_rows(&format!(
                "SELECT trace_id, ts_ns, duration_ns / 1000000.0 AS duration_ms \
                 FROM spans WHERE {span_where} ORDER BY ts_ns LIMIT 1000"
            ))
            .map_err(|e| e.to_string())?;
        let pct = reader
            .query_json_rows(&format!(
                "SELECT quantile_cont(duration_ns, 0.5) / 1000000.0 AS p50, \
                 quantile_cont(duration_ns, 0.95) / 1000000.0 AS p95, \
                 quantile_cont(duration_ns, 0.99) / 1000000.0 AS p99 \
                 FROM spans WHERE {span_where}"
            ))
            .map_err(|e| e.to_string())?;
        let pct = pct.into_iter().next().unwrap_or_else(|| json!({}));
        Ok(json!({
            "points": points,
            "p50_ms": pct.get("p50").cloned().unwrap_or(Value::Null),
            "p95_ms": pct.get("p95").cloned().unwrap_or(Value::Null),
            "p99_ms": pct.get("p99").cloned().unwrap_or(Value::Null),
        }))
    })
    .await?;
    Ok(Json(body))
}

async fn trace_detail(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    if id.chars().count() > 200 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "trace id too long"})),
        )
            .into_response());
    }
    let body = with_reader(&state.db_path, move |reader| {
        let id_sql = sql_string(&id);
        let spans = reader
            .query_json_rows(&format!(
                "SELECT ts_ns, trace_id, span_id, parent_span_id, span_name, span_kind, \
                 duration_ns, status_code, status_message, service_name, \
                 experiment_id, experiment_name, fault_type, fault_subtype, \
                 span_attrs, events \
                 FROM spans WHERE trace_id = {id_sql} ORDER BY ts_ns LIMIT 2000"
            ))
            .map_err(|e| e.to_string())?;
        if spans.is_empty() {
            return Ok(None);
        }
        let logs = reader
            .query_json_rows(&format!(
                "SELECT ts_ns, severity_text, body, trace_id, span_id, log_attrs \
                 FROM logs WHERE trace_id = {id_sql} ORDER BY ts_ns LIMIT 1000"
            ))
            .map_err(|e| e.to_string())?;
        Ok(Some(json!({
            "trace_id": id,
            "spans": spans,
            "logs": logs,
        })))
    })
    .await?;
    match body {
        Some(body) => Ok(Json(body)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "trace not found"})),
        )
            .into_response()),
    }
}

// ---------------------------------------------------------------------------
// GET /api/metrics/catalog + /api/metrics/query

/// Raw metric tables and their UI-facing type names, in preference order
/// (a name reused across tables resolves to the first match).
const METRIC_TABLES: [(&str, &str); 3] = [
    ("metric_sums", "sum"),
    ("metric_gauges", "gauge"),
    ("metric_histograms", "histogram"),
];

/// Catalog of raw metrics: name → table types it appears in + attribute
/// keys seen on its points (sampled, capped per table).
fn load_catalog(reader: &Reader) -> Result<Vec<Value>, String> {
    let mut names: std::collections::BTreeMap<
        String,
        (Vec<String>, std::collections::BTreeSet<String>),
    > = std::collections::BTreeMap::new();
    for (table, kind) in METRIC_TABLES {
        let rows = reader
            .query_json_rows(&format!(
                "SELECT metric_name, map_keys(attrs) AS k FROM {table} LIMIT 5000"
            ))
            .map_err(|e| e.to_string())?;
        for row in rows {
            let Some(name) = row.get("metric_name").and_then(Value::as_str) else {
                continue;
            };
            let entry = names.entry(name.to_string()).or_default();
            let kind = kind.to_string();
            if !entry.0.contains(&kind) {
                entry.0.push(kind);
            }
            if let Some(keys) = row.get("k").and_then(Value::as_array) {
                for key in keys.iter().filter_map(Value::as_str) {
                    entry.1.insert(key.to_string());
                }
            }
        }
    }
    Ok(names
        .into_iter()
        .map(|(name, (types, dims))| {
            json!({
                "name": name,
                "types": types,
                "dimensions": dims.into_iter().collect::<Vec<_>>(),
            })
        })
        .collect())
}

async fn metrics_catalog(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let metrics = with_reader(&state.db_path, load_catalog).await?;
    Ok(Json(json!({"metrics": metrics})))
}

/// Attribute keys may become part of a SQL expression, so they get the
/// same strict charset as metric identifiers (the metrics crate's own
/// check is private).
fn valid_attr_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 100
        && key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.')
}

/// Approximate a quantile from histogram bucket counts with linear
/// interpolation inside the target bucket. `bounds` has one fewer entry
/// than `counts` (OTel explicit bounds); a quantile landing in the
/// overflow bucket clamps to the last explicit bound.
fn hist_quantile(counts: &[f64], bounds: &[f64], q: f64) -> Option<f64> {
    let total: f64 = counts.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let target = q * total;
    let mut cum = 0.0;
    for (i, &c) in counts.iter().enumerate() {
        if c <= 0.0 {
            continue;
        }
        if cum + c >= target {
            let lo = if i == 0 {
                0.0
            } else {
                bounds.get(i - 1).copied().unwrap_or(0.0)
            };
            let hi = bounds
                .get(i)
                .copied()
                .unwrap_or_else(|| bounds.last().copied().unwrap_or(lo));
            return Some(lo + (target - cum) / c * (hi - lo));
        }
        cum += c;
    }
    bounds.last().copied()
}

#[derive(Deserialize)]
struct MetricQueryParams {
    name: Option<String>,
    group_by: Option<String>,
    range: Option<String>,
    interval: Option<String>,
}

/// Running aggregation for one histogram (bucket, group) cell.
#[derive(Default)]
struct HistAcc {
    count: u64,
    sum: f64,
    buckets: Vec<f64>,
    bounds: Vec<f64>,
}

async fn metrics_query(
    State(state): State<ApiState>,
    Query(params): Query<MetricQueryParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let Some(name) = params.name.filter(|n| !n.is_empty()) else {
        return Err(bad("missing query parameter: name".into()));
    };
    if name.len() > 200 {
        return Err(bad("metric name too long".into()));
    }
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
    let group_by = match params.group_by.as_deref().filter(|g| !g.is_empty()) {
        Some(g) if valid_attr_key(g) => Some(g.to_string()),
        Some(g) => {
            return Err(bad(format!(
                "invalid group_by {g:?}; expected lowercase [a-z0-9_.]+"
            )))
        }
        None => None,
    };

    let metric_name = name.clone();
    let body = with_reader(&state.db_path, move |reader| {
        let catalog = load_catalog(reader)?;
        let Some(entry) = catalog
            .iter()
            .find(|m| m.get("name").and_then(Value::as_str) == Some(metric_name.as_str()))
        else {
            return Ok(None);
        };
        let types: Vec<&str> = entry
            .get("types")
            .and_then(Value::as_array)
            .map(|t| t.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let Some((table, kind)) = METRIC_TABLES.iter().find(|(_, k)| types.contains(k)) else {
            return Ok(None);
        };
        let grp_sel = group_by.as_ref().map_or(String::new(), |g| {
            format!(", attrs[{}] AS grp", sql_string(g))
        });
        let window = w(cur.0, cur.1);
        let name_sql = sql_string(&metric_name);

        // Pivot (bucket, group) rows into one series per group.
        let mut groups: std::collections::BTreeMap<Option<String>, Vec<Value>> =
            std::collections::BTreeMap::new();
        if *kind == "histogram" {
            // Fetch raw rows and aggregate in Rust: elementwise bucket sums
            // per (bucket, group), then interpolate p95. Bounds are assumed
            // stable per metric — mixed schemas degrade to first-seen.
            let rows = reader
                .query_json_rows(&format!(
                    "SELECT {} AS ts, count, sum, bucket_counts, explicit_bounds{grp_sel} \
                     FROM {table} WHERE metric_name = {name_sql} AND {window} \
                     ORDER BY 1 LIMIT 10000",
                    bucket_expr(bucket_s)
                ))
                .map_err(|e| e.to_string())?;
            let mut cells: std::collections::BTreeMap<(i64, Option<String>), HistAcc> =
                std::collections::BTreeMap::new();
            for row in &rows {
                let Some(ts) = row.get("ts").and_then(Value::as_i64) else {
                    continue;
                };
                let grp = row.get("grp").and_then(Value::as_str).map(str::to_owned);
                let acc = cells.entry((ts, grp)).or_default();
                acc.count += row.get("count").and_then(Value::as_u64).unwrap_or(0);
                acc.sum += row.get("sum").and_then(Value::as_f64).unwrap_or(0.0);
                if let Some(bc) = row.get("bucket_counts").and_then(Value::as_array) {
                    for (i, b) in bc.iter().enumerate() {
                        let v = b.as_f64().unwrap_or(0.0);
                        if i < acc.buckets.len() {
                            acc.buckets[i] += v;
                        } else {
                            acc.buckets.push(v);
                        }
                    }
                }
                if acc.bounds.is_empty() {
                    if let Some(eb) = row.get("explicit_bounds").and_then(Value::as_array) {
                        acc.bounds = eb.iter().filter_map(Value::as_f64).collect();
                    }
                }
            }
            for ((ts, grp), acc) in cells {
                if acc.count == 0 {
                    continue;
                }
                groups.entry(grp).or_default().push(json!({
                    "ts": ts,
                    "avg": acc.sum / acc.count as f64,
                    "p95": hist_quantile(&acc.buckets, &acc.bounds, 0.95),
                }));
            }
        } else {
            let agg = if *kind == "sum" {
                "SUM(value)"
            } else {
                "AVG(value)"
            };
            let grp_by = if group_by.is_some() { ", 3" } else { "" };
            let rows = reader
                .query_json_rows(&format!(
                    "SELECT {} AS ts, {agg} AS v{grp_sel} FROM {table} \
                     WHERE metric_name = {name_sql} AND {window} GROUP BY 1{grp_by} ORDER BY 1",
                    bucket_expr(bucket_s)
                ))
                .map_err(|e| e.to_string())?;
            for row in rows {
                let Some(ts) = row.get("ts").and_then(Value::as_i64) else {
                    continue;
                };
                let grp = row.get("grp").and_then(Value::as_str).map(str::to_owned);
                groups.entry(grp).or_default().push(json!({
                    "ts": ts,
                    "v": row.get("v").cloned().unwrap_or(Value::Null),
                }));
            }
        }

        let series: Vec<Value> = groups
            .into_iter()
            .map(|(grp, mut points)| {
                points.sort_by_key(|p| p.get("ts").and_then(Value::as_i64).unwrap_or(0));
                json!({"group": grp, "points": points})
            })
            .collect();
        Ok(Some(json!({
            "name": metric_name,
            "type": kind,
            "interval": interval,
            "range": range,
            "group_by": group_by,
            "series": series,
        })))
    })
    .await?;
    match body {
        Some(body) => Ok(Json(body)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("metric {name:?} not found; see /api/metrics/catalog")})),
        )
            .into_response()),
    }
}

// ---------------------------------------------------------------------------
// GET /api/topology

#[derive(Deserialize)]
struct TopologyParams {
    range: Option<String>,
}

/// tumult tags spans with the system under test under this attribute.
const TARGET_ATTR: &str = "span_attrs['resilience.target.name']";

async fn topology(
    State(state): State<ApiState>,
    Query(params): Query<TopologyParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let mut window = String::new();
    let mut cwindow = String::new();
    if let Some(range) = &params.range {
        let Some((cur, _)) = windows(range) else {
            return Err(bad(format!("invalid range {range:?}; expected 24h|7d|14d")));
        };
        window = format!(" AND {}", w(cur.0, cur.1));
        // Edge queries join spans to itself — qualify the window there.
        cwindow = format!(" AND c.ts_ns >= {} AND c.ts_ns < {}", cur.0, cur.1);
    }
    let body = with_reader(&state.db_path, move |reader| {
        let query = |sql: &str| reader.query_json_rows(sql).map_err(|e| e.to_string());
        // Nodes: one per service and one per tumult target, with health
        // aggregates over their spans.
        let mut nodes = query(&format!(
            "SELECT service_name AS name, 'service' AS type, COUNT(*) AS runs, \
             COUNT(*) FILTER (WHERE status_code = 'Error') AS errors, \
             AVG(duration_ns) AS avg_duration_ns \
             FROM spans WHERE service_name <> ''{window} GROUP BY 1"
        ))?;
        nodes.extend(query(&format!(
            "SELECT {TARGET_ATTR} AS name, 'target' AS type, COUNT(*) AS runs, \
             COUNT(*) FILTER (WHERE status_code = 'Error') AS errors, \
             AVG(duration_ns) AS avg_duration_ns \
             FROM spans WHERE {TARGET_ATTR} IS NOT NULL{window} GROUP BY 1"
        ))?);

        // Edges: parent→child service hops (a differing span name keeps
        // intra-service calls visible as self-loops), plus service→target
        // calls from the target attribute.
        let mut edges = query(&format!(
            "SELECT 'svc:' || p.service_name AS from_id, 'svc:' || c.service_name AS to_id, \
             COUNT(*) AS weight \
             FROM spans c JOIN spans p ON c.parent_span_id = p.span_id \
             WHERE (p.service_name <> c.service_name OR p.span_name <> c.span_name){cwindow} \
             GROUP BY 1, 2"
        ))?;
        edges.extend(query(&format!(
            "SELECT 'svc:' || service_name AS from_id, \
             'tgt:' || {TARGET_ATTR} AS to_id, COUNT(*) AS weight \
             FROM spans WHERE {TARGET_ATTR} IS NOT NULL{window} GROUP BY 1, 2"
        ))?);

        // Busiest first, capped at 100 nodes; edges to trimmed nodes drop.
        nodes
            .sort_by_key(|n| std::cmp::Reverse(n.get("runs").and_then(Value::as_u64).unwrap_or(0)));
        nodes.truncate(100);
        let nodes: Vec<Value> = nodes
            .into_iter()
            .filter_map(|n| {
                let name = n.get("name").and_then(Value::as_str)?;
                let kind = n.get("type").and_then(Value::as_str)?;
                let prefix = if kind == "service" { "svc" } else { "tgt" };
                Some(json!({
                    "id": format!("{prefix}:{name}"),
                    "name": name,
                    "type": kind,
                    "runs": n.get("runs").cloned().unwrap_or(Value::Null),
                    "errors": n.get("errors").cloned().unwrap_or(Value::Null),
                    "avg_duration_ns": n.get("avg_duration_ns").cloned().unwrap_or(Value::Null),
                }))
            })
            .collect();
        let ids: std::collections::HashSet<&str> = nodes
            .iter()
            .filter_map(|n| n.get("id").and_then(Value::as_str))
            .collect();
        let edges: Vec<Value> = edges
            .into_iter()
            .filter(|e| {
                let from = e.get("from_id").and_then(Value::as_str).unwrap_or("");
                let to = e.get("to_id").and_then(Value::as_str).unwrap_or("");
                ids.contains(from) && ids.contains(to)
            })
            .collect();
        Ok(json!({"nodes": nodes, "edges": edges}))
    })
    .await?;
    Ok(Json(body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hist_quantile_interpolates_within_bucket() {
        // 4 observations: 1 below 100, 2 in [100,200), 1 at/above 200.
        let counts = [1.0, 2.0, 1.0];
        let bounds = [100.0, 200.0];
        // Median falls halfway through the middle bucket.
        assert_eq!(hist_quantile(&counts, &bounds, 0.5), Some(150.0));
        // p30 lands just inside the middle bucket.
        assert_eq!(hist_quantile(&counts, &bounds, 0.3), Some(110.0));
        // p95 lands in the overflow bucket → clamps to the last bound.
        assert_eq!(hist_quantile(&counts, &bounds, 0.95), Some(200.0));
    }

    #[test]
    fn hist_quantile_handles_empty_and_zero_buckets() {
        assert_eq!(hist_quantile(&[], &[], 0.5), None);
        assert_eq!(hist_quantile(&[0.0, 0.0], &[100.0], 0.5), None);
        // Zero-count leading buckets are skipped; the target then lands in
        // the overflow bucket and clamps to the last bound.
        assert_eq!(hist_quantile(&[0.0, 4.0], &[100.0], 0.5), Some(100.0));
    }

    #[test]
    fn attr_key_charset_is_strict() {
        assert!(valid_attr_key("route"));
        assert!(valid_attr_key("http.route_v2"));
        assert!(!valid_attr_key(""));
        assert!(!valid_attr_key("Route"));
        assert!(!valid_attr_key("x';DROP"));
        assert!(!valid_attr_key("a b"));
    }
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

#[derive(Deserialize)]
struct GenerateRequest {
    metric: String,
}

/// `POST /api/reports/generate {metric}` — manual counterpart to the
/// scheduler: render one metric digest now (over all stored data, matching
/// `GET /report?metric=`), write it into the reports dir so it appears in
/// `GET /api/reports`, and return its name. Manual digests carry a
/// `manual_<metric>_<epoch>.html` name, distinct from the scheduler's
/// `report_<epoch>.html`.
async fn generate_report(
    State(state): State<ApiState>,
    Json(req): Json<GenerateRequest>,
) -> Result<Json<Value>, Response> {
    let metric = req.metric.trim().to_string();
    if metric.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "metric must not be empty"})),
        )
            .into_response());
    }
    let metrics_dir = state.metrics_dir.as_ref().clone();
    let reports_dir = state.reports_dir.as_ref().clone();
    let llm = state.llm.clone();
    let metric_name = metric.clone();
    let body = with_reader(&state.db_path, move |reader| {
        let defs =
            tumult_metrics::load_dir(&metrics_dir).map_err(|e| format!("load metrics: {e}"))?;
        let Some(def) = defs.iter().find(|d| d.name == metric_name) else {
            return Ok(None);
        };
        let report = tumult_report::build_report(
            reader,
            std::slice::from_ref(def),
            &format!("Krönika — {metric_name}"),
            None,
        )
        .map_err(|e| e.to_string())?;
        Ok(Some(report))
    })
    .await?;
    let Some(report) = body else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": format!("metric {metric:?} not found; see /api/metrics")})),
        )
            .into_response());
    };
    // Best-effort LLM narrative: unreachable/unconfigured LLM or a reply
    // with no grounded sentences leaves the digest unchanged.
    let report =
        tumult_report::narrative::narrate(&llm, report, std::time::Duration::from_secs(30)).await;
    let html = tumult_report::render_html(&report);
    std::fs::create_dir_all(&reports_dir).map_err(|e| internal(e.to_string()))?;
    let now_s = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let name = format!("manual_{metric}_{now_s}.html");
    std::fs::write(reports_dir.join(&name), &html).map_err(|e| internal(e.to_string()))?;
    Ok(Json(
        json!({"name": name, "metric": metric, "bytes": html.len()}),
    ))
}

// ---------------------------------------------------------------------------
// GET /api/scores + /api/reports/v2/*

#[derive(Deserialize)]
struct ScoresQuery {
    range: Option<String>,
}

/// `GET /api/scores?range=24h|7d|14d` — resilience scorecard as of now,
/// with the portfolio delta against the previous equal window.
async fn scores(
    State(state): State<ApiState>,
    Query(q): Query<ScoresQuery>,
) -> Result<Json<Value>, Response> {
    let range = q.range.as_deref().unwrap_or("7d");
    let Some(((from, to), _)) = windows(range) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "range must be one of 24h|7d|14d"})),
        )
            .into_response());
    };
    let card = with_reader(&state.db_path, move |reader| {
        tumult_compliance::scoring::compute(reader, to, Some(to - from))
    })
    .await?;
    Ok(Json(
        serde_json::to_value(card).map_err(|e| internal(e.to_string()))?,
    ))
}

#[derive(Deserialize)]
struct TreeParams {
    node: Option<String>,
    range: Option<String>,
}

/// `GET /api/scores/tree?node=<path>&range=24h|7d|14d` — org rollup for one
/// node: criticality-weighted score recomputed from all leaves in its
/// subtree, coverage, a period sparkline, and one level of child rollups.
async fn scores_tree(
    State(state): State<ApiState>,
    Query(params): Query<TreeParams>,
) -> Result<Json<Value>, Response> {
    let node = params.node.unwrap_or_default();
    let node = node.trim_matches('/').to_string();
    if state.org.resolve(&node).is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("unknown org node {node:?}")})),
        )
            .into_response());
    }
    let Some(secs) = parse_range(params.range.as_deref().unwrap_or("7d")) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "range must be one of 24h|7d|14d"})),
        )
            .into_response());
    };
    let period_ns = secs * 1_000_000_000;
    let as_of = now_ns();
    let org = state.org.clone();

    let payload = with_reader(&state.db_path, move |reader| {
        // Leaves at an instant: every scored experiment plus pending manual
        // records (expected but unscored). Pending status is read as of NOW
        // for every sample point — a documented approximation, since the
        // lifecycle has no history before the audit trail.
        let leaves_at = |t: i64| -> Result<Vec<tumult_compliance::ScoredLeaf>, String> {
            let card = tumult_compliance::scoring::compute(reader, t, None)?;
            let mut leaves: Vec<tumult_compliance::ScoredLeaf> = card
                .experiments
                .iter()
                .map(|e| tumult_compliance::ScoredLeaf {
                    name: e.name.clone(),
                    score: Some(e.score),
                })
                .collect();
            leaves.extend(
                tumult_compliance::scoring::pending_manual_leaves(reader)?
                    .into_iter()
                    .map(|name| tumult_compliance::ScoredLeaf { name, score: None }),
            );
            Ok(leaves)
        };

        let current = org
            .compute_node(&node, &leaves_at(as_of)?)
            .ok_or_else(|| format!("unknown org node {node:?}"))?;
        let previous = org
            .compute_node(&node, &leaves_at(as_of - period_ns)?)
            .ok_or_else(|| format!("unknown org node {node:?}"))?;

        const POINTS: i64 = 10;
        let step = period_ns / POINTS;
        let mut sparkline = Vec::with_capacity(POINTS as usize);
        for i in 1..=POINTS {
            let t = as_of - period_ns + step * i;
            let score = org
                .compute_node(&node, &leaves_at(t)?)
                .map_or(0.0, |n| n.score);
            sparkline.push(vec![json!(i), json!(score)]);
        }

        Ok(json!({
            "path": current.path,
            "name": current.name,
            "kind": current.kind,
            "score": current.score,
            "band": current.band,
            "delta": current.score - previous.score,
            "coverage": current.coverage,
            "scored": current.scored,
            "expected": current.expected,
            "weakest": current.weakest,
            "weight": current.weight,
            "sparkline": sparkline,
            "children": current.children,
        }))
    })
    .await?;
    Ok(Json(payload))
}

#[derive(Deserialize)]
struct GenerateV2Request {
    #[serde(rename = "type")]
    kind: String,
    period: Option<String>,
    experiment_id: Option<String>,
    framework: Option<String>,
}

/// `POST /api/reports/v2/generate` — build one compliance-grade report and
/// persist `{id}.pdf`, `{id}.html` and `{id}.json` under `reports/v2/`.
async fn generate_report_v2(
    State(state): State<ApiState>,
    Json(req): Json<GenerateV2Request>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let Ok(kind) = serde_json::from_value::<tumult_compliance::TemplateKind>(json!(req.kind))
    else {
        return Err(bad(format!(
            "unknown type {:?}; expected executive-digest|game-day|evidence-pack",
            req.kind
        )));
    };
    let period_ns = match req.period.as_deref() {
        None => 7 * 86_400 * 1_000_000_000i64,
        Some(p) => match parse_range(p) {
            Some(secs) => secs * 1_000_000_000,
            None => return Err(bad("period must be one of 24h|7d|14d".into())),
        },
    };
    if kind == tumult_compliance::TemplateKind::GameDay
        && req.experiment_id.as_deref().is_none_or(str::is_empty)
    {
        return Err(bad("game-day requires experiment_id".into()));
    }
    if kind == tumult_compliance::TemplateKind::EvidencePack {
        match req.framework.as_deref() {
            None => return Err(bad("evidence-pack requires framework".into())),
            Some(f)
                if !tumult_compliance::builders::FRAMEWORK_CLAUSES
                    .iter()
                    .any(|(name, _)| *name == f.to_ascii_lowercase()) =>
            {
                return Err(bad(format!(
                    "unknown framework {f:?}; expected dora|nis2|iso27001|soc2"
                )));
            }
            _ => {}
        }
    }

    let generated_at = now_ns();
    let exp_id = req.experiment_id.clone();
    let framework = req.framework.clone();
    let org = state.org.clone();
    let built = with_reader(&state.db_path, move |reader| match kind {
        tumult_compliance::TemplateKind::ExecutiveDigest => {
            tumult_compliance::builders::build_executive(
                reader,
                &org,
                generated_at,
                period_ns,
                generated_at,
            )
            .map(Some)
        }
        tumult_compliance::TemplateKind::GameDay => tumult_compliance::builders::build_game_day(
            reader,
            exp_id.as_deref().unwrap_or_default(),
            generated_at,
        ),
        tumult_compliance::TemplateKind::EvidencePack => {
            tumult_compliance::builders::build_evidence_pack(
                reader,
                framework.as_deref().unwrap_or_default(),
                Some(period_ns),
                generated_at,
            )
            .map(Some)
        }
    })
    .await?;
    let Some(doc) = built else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "experiment_id not found"})),
        )
            .into_response());
    };

    let pdf =
        tumult_compliance::typst_pdf::render_pdf(&doc).map_err(|e| internal(e.to_string()))?;
    let html = tumult_compliance::html::render(&doc);
    let sha256: String = {
        use sha2::Digest as _;
        sha2::Sha256::digest(&pdf)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    };

    let v2_dir = state.reports_dir.join("v2");
    std::fs::create_dir_all(&v2_dir).map_err(|e| internal(e.to_string()))?;
    let id = &doc.meta.doc_id;
    let meta = json!({
        "doc_id": id,
        "type": req.kind,
        "title": doc.meta.title,
        "created_ns": doc.meta.generated_at_ns,
        "data_as_of_ns": doc.meta.data_as_of_ns,
        "bytes": pdf.len(),
        "sha256": sha256,
        "params": {
            "period": req.period,
            "experiment_id": req.experiment_id,
            "framework": req.framework,
        },
    });
    std::fs::write(v2_dir.join(format!("{id}.pdf")), &pdf).map_err(|e| internal(e.to_string()))?;
    std::fs::write(v2_dir.join(format!("{id}.html")), &html)
        .map_err(|e| internal(e.to_string()))?;
    std::fs::write(
        v2_dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&meta).unwrap_or_default(),
    )
    .map_err(|e| internal(e.to_string()))?;
    Ok(Json(meta))
}

/// `GET /api/reports/v2` — metas of every generated v2 report, newest first.
async fn list_reports_v2(State(state): State<ApiState>) -> Json<Value> {
    let mut reports = Vec::new();
    if let Ok(entries) = std::fs::read_dir(state.reports_dir.join("v2")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(meta) = serde_json::from_str::<Value>(&text) {
                        reports.push(meta);
                    }
                }
            }
        }
    }
    reports.sort_by_key(|r| r.get("created_ns").and_then(Value::as_i64).unwrap_or(0));
    reports.reverse();
    Json(json!({"reports": reports}))
}

/// A doc id is `KRK-<code>-<yyyymmdd>-<hash6>`: flat, safe charset.
fn valid_doc_id(id: &str) -> bool {
    id.starts_with("KRK-")
        && id.len() <= 100
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

async fn get_report_v2_pdf(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    if !valid_doc_id(&id) {
        return (StatusCode::BAD_REQUEST, "invalid document id").into_response();
    }
    match std::fs::read(state.reports_dir.join("v2").join(format!("{id}.pdf"))) {
        Ok(bytes) => (
            [(axum::http::header::CONTENT_TYPE, "application/pdf")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "report not found").into_response(),
    }
}

async fn get_report_v2_html(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    if !valid_doc_id(&id) {
        return (StatusCode::BAD_REQUEST, "invalid document id").into_response();
    }
    match std::fs::read_to_string(state.reports_dir.join("v2").join(format!("{id}.html"))) {
        Ok(html) => Html(html).into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "report not found").into_response(),
    }
}
