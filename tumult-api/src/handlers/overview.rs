//! `GET /api/overview` — KPI cards, experiments per day, target
//! leaderboard, fault breakdown.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::sql_util::{
    and_pred, bucket_expr, env_metric_exists, env_scope_where, env_trace_exists, ratio_series,
    scalar, series, w, windows, with_reader,
};
use crate::ApiState;

/// Span-name predicate selecting one row per experiment run.
const ROOT: &str = "span_name = 'resilience.experiment'";

// ---------------------------------------------------------------------------
// GET /api/overview

#[derive(Deserialize)]
pub(crate) struct OverviewParams {
    range: Option<String>,
}

/// KPI single-value query builder: (from_ns, to_ns, env scopes) → SQL.
type KpiValueSql = fn(i64, i64, &[String]) -> String;

/// KPI sparkline query builder: (from_ns, to_ns, bucket_s, env scopes) → SQL.
type KpiSparkSql = fn(i64, i64, i64, &[String]) -> String;

/// SQL building blocks for one KPI card: single-value query per window, and
/// a bucketed sparkline query (either one series or a num/den pair). Every
/// query takes the principal's environment scopes and filters its rows.
struct Kpi {
    name: &'static str,
    label: &'static str,
    /// `"count"`, `"ratio"` or `"seconds"` — the UI formats accordingly.
    unit: &'static str,
    value: KpiValueSql,
    spark_num: KpiSparkSql,
    spark_den: Option<KpiSparkSql>,
}

fn sql_experiments_value(f: i64, t: i64, scopes: &[String]) -> String {
    format!(
        "SELECT COUNT(*) AS v FROM spans WHERE {ROOT} AND {}{}",
        w(f, t),
        and_pred(env_trace_exists("spans", scopes))
    )
}

fn sql_experiments_spark(f: i64, t: i64, b: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS ts, COUNT(*) AS v FROM spans WHERE {ROOT} AND {}{} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t),
        and_pred(env_trace_exists("spans", scopes))
    )
}

fn sql_pass_rate_value(f: i64, t: i64, scopes: &[String]) -> String {
    format!(
        "SELECT SUM(CAST(value AS DOUBLE)) FILTER (WHERE outcome_status = 'success') \
         / NULLIF(SUM(CAST(value AS DOUBLE)), 0) AS v \
         FROM metric_sums WHERE metric_name = 'tumult.experiments.total' AND {}{}",
        w(f, t),
        and_pred(env_metric_exists("metric_sums", scopes))
    )
}

fn sql_pass_num(f: i64, t: i64, b: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS ts, SUM(CAST(value AS DOUBLE)) AS v FROM metric_sums \
         WHERE metric_name = 'tumult.experiments.total' AND outcome_status = 'success' AND {}{} \
         GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t),
        and_pred(env_metric_exists("metric_sums", scopes))
    )
}

fn sql_pass_den(f: i64, t: i64, b: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS ts, SUM(CAST(value AS DOUBLE)) AS v FROM metric_sums \
         WHERE metric_name = 'tumult.experiments.total' AND {}{} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t),
        and_pred(env_metric_exists("metric_sums", scopes))
    )
}

fn sql_deviation_value(f: i64, t: i64, scopes: &[String]) -> String {
    let window = w(f, t);
    let metrics_env = and_pred(env_metric_exists("metric_sums", scopes));
    let spans_env = and_pred(env_trace_exists("spans", scopes));
    format!(
        "SELECT (SELECT COALESCE(SUM(CAST(value AS DOUBLE)), 0) FROM metric_sums \
         WHERE metric_name = 'tumult.hypothesis.deviations.total' AND {window}{metrics_env}) \
         / NULLIF((SELECT COUNT(*) FROM spans WHERE {ROOT} AND {window}{spans_env}), 0) AS v"
    )
}

fn sql_deviation_num(f: i64, t: i64, b: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS ts, COALESCE(SUM(CAST(value AS DOUBLE)), 0) AS v FROM metric_sums \
         WHERE metric_name = 'tumult.hypothesis.deviations.total' AND {}{} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t),
        and_pred(env_metric_exists("metric_sums", scopes))
    )
}

fn sql_mttr_value(f: i64, t: i64, scopes: &[String]) -> String {
    format!(
        "SELECT AVG(recovery_time_s) AS v FROM spans WHERE recovery_time_s IS NOT NULL AND {}{}",
        w(f, t),
        and_pred(env_trace_exists("spans", scopes))
    )
}

fn sql_mttr_spark(f: i64, t: i64, b: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS ts, AVG(recovery_time_s) AS v FROM spans \
         WHERE recovery_time_s IS NOT NULL AND {}{} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t),
        and_pred(env_trace_exists("spans", scopes))
    )
}

fn sql_coverage_value(f: i64, t: i64, scopes: &[String]) -> String {
    format!(
        "SELECT COUNT(DISTINCT target_system) AS v FROM spans WHERE target_system IS NOT NULL AND {}{}",
        w(f, t),
        and_pred(env_trace_exists("spans", scopes))
    )
}

fn sql_coverage_spark(f: i64, t: i64, b: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS ts, COUNT(DISTINCT target_system) AS v FROM spans \
         WHERE target_system IS NOT NULL AND {}{} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t),
        and_pred(env_trace_exists("spans", scopes))
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

pub(crate) async fn overview(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
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
    let scopes = principal.env_scopes.clone();

    let body = with_reader(&state.db_path, move |reader| {
        let mut kpis = Vec::new();
        for kpi in KPIS {
            let value = scalar(reader, &(kpi.value)(cur.0, cur.1, &scopes))?;
            let prev_value = scalar(reader, &(kpi.value)(prev.0, prev.1, &scopes))?;
            let delta = value.zip(prev_value).map(|(v, p)| v - p);
            let num = series(reader, &(kpi.spark_num)(cur.0, cur.1, bucket_s, &scopes))?;
            let spark = match kpi.spark_den {
                Some(den_fn) => {
                    let den = series(reader, &den_fn(cur.0, cur.1, bucket_s, &scopes))?;
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
                "SELECT {} AS ts, COUNT(*) AS v FROM spans WHERE {ROOT} AND {}{} \
                 GROUP BY 1 ORDER BY 1",
                bucket_expr(86_400),
                w(cur.0, cur.1),
                and_pred(env_trace_exists("spans", &scopes))
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
                   AND s.ts_ns >= {} AND s.ts_ns < {}{} \
                 GROUP BY 1 ORDER BY experiments DESC LIMIT 10",
                cur.0,
                cur.1,
                and_pred(env_scope_where("s.target_environment", &scopes))
            ))
            .map_err(|e| e.to_string())?;

        let faults = reader
            .query_json_rows(&format!(
                "SELECT fault_type, fault_subtype, COUNT(*) AS count FROM spans \
                 WHERE fault_type IS NOT NULL AND {}{} \
                 GROUP BY 1, 2 ORDER BY count DESC LIMIT 10",
                w(cur.0, cur.1),
                and_pred(env_trace_exists("spans", &scopes))
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
