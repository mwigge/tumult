//! `GET /api/overview` — KPI cards, experiments per day, target
//! leaderboard, fault injection steps.

use crate::error::ApiError;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_lake::scope;

use crate::auth::Principal;
use crate::sql_util::{
    bucket_expr, bucket_of, env_trace_exists, invalid_range, ratio_series, run_pass_rate_sql,
    scalar, series, w, windows, with_reader,
};
use crate::ApiState;

/// Span-name predicate selecting the root span of a run.
const ROOT: &str = "span_name = 'resilience.experiment'";

/// The span the runner opens over the rollback of a run; the steps of the
/// rollback are its children.
const ROLLBACK: &str = "resilience.rollback";

/// Environment predicate for `run_roots`: a root names its own environment.
fn and_root_env(scopes: &[String]) -> String {
    scope::and(scope::in_list("target_environment", scopes))
}

/// The roots `s` of the runs that started in `[f, t)` in the scopes, as a
/// predicate on `run_roots`.
fn roots_in(f: i64, t: i64, scopes: &[String]) -> String {
    format!(
        "s.ts_ns >= {f} AND s.ts_ns < {t}{}",
        scope::and(scope::in_list("s.target_environment", scopes))
    )
}

/// The runs that started in `[f, t)` in the scopes, as the FROM and WHERE
/// of a query: each root `s` with its completion log `l`, for
/// [`run_pass_rate_sql`]. A run completes after it starts, so only the
/// logs since `f` are read.
fn runs_from(f: i64, t: i64, scopes: &[String]) -> String {
    format!(
        "FROM run_roots AS s \
         LEFT JOIN ({}) AS l ON s.experiment_id = l.experiment_id \
         WHERE {}",
        tumult_lake::run_completions_sql(Some(f)),
        roots_in(f, t, scopes)
    )
}

/// The traced runs that started in `[f, t)` in the scopes, as a predicate
/// on `spans`. The deviation counter comes only with a trace, so a run
/// stored only as a journal is not one of them.
fn traced_runs(f: i64, t: i64, scopes: &[String]) -> String {
    format!(
        "{ROOT} AND {}{}",
        w(f, t),
        scope::and(env_trace_exists("spans", scopes))
    )
}

/// The fault injection steps of the runs in `[f, t)` in the scopes, per
/// fault type and subtype, for the pie. The runner marks an action
/// "injection" and a probe sample "observation", and sets no subtype: the
/// plugin of the step takes its place. A step of the rollback undoes a
/// fault and is left out. A span of a manual import counts with the type
/// and subtype it was given.
fn sql_faults(f: i64, t: i64, scopes: &[String]) -> String {
    format!(
        "SELECT s.fault_type, COALESCE(s.fault_subtype, s.plugin_name) AS fault_subtype, \
         COUNT(*) AS count FROM spans AS s \
         WHERE s.fault_type IS NOT NULL AND s.fault_type <> 'observation' \
         AND NOT EXISTS (SELECT 1 FROM spans AS r WHERE r.trace_id = s.trace_id \
         AND r.span_id = s.parent_span_id AND r.span_name = '{ROLLBACK}') \
         AND s.ts_ns >= {f} AND s.ts_ns < {t}{} \
         GROUP BY 1, 2 ORDER BY 3 DESC LIMIT 10",
        scope::and(env_trace_exists("s", scopes))
    )
}

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
        "SELECT COUNT(*) AS v FROM run_roots WHERE {}{}",
        w(f, t),
        and_root_env(scopes)
    )
}

fn sql_experiments_spark(f: i64, t: i64, b: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS ts, COUNT(*) AS v FROM run_roots WHERE {}{} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t),
        and_root_env(scopes)
    )
}

fn sql_pass_rate_value(f: i64, t: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS v {}",
        run_pass_rate_sql("s", "l"),
        runs_from(f, t, scopes)
    )
}

fn sql_pass_rate_spark(f: i64, t: i64, b: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS ts, {} AS v {} GROUP BY 1 ORDER BY 1",
        bucket_of("s.ts_ns", b),
        run_pass_rate_sql("s", "l"),
        runs_from(f, t, scopes)
    )
}

fn sql_deviation_value(f: i64, t: i64, scopes: &[String]) -> String {
    let window = w(f, t);
    let metrics_env = scope::and(scope::table_predicate("metric_sums", scopes));
    format!(
        "SELECT (SELECT COALESCE(SUM(CAST(value AS DOUBLE)), 0) FROM metric_sums \
         WHERE metric_name = 'tumult.hypothesis.deviations.total' AND {window}{metrics_env}) \
         / NULLIF((SELECT COUNT(*) FROM spans WHERE {}), 0) AS v",
        traced_runs(f, t, scopes)
    )
}

fn sql_traced_runs_spark(f: i64, t: i64, b: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS ts, COUNT(*) AS v FROM spans WHERE {} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        traced_runs(f, t, scopes)
    )
}

fn sql_deviation_num(f: i64, t: i64, b: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS ts, COALESCE(SUM(CAST(value AS DOUBLE)), 0) AS v FROM metric_sums \
         WHERE metric_name = 'tumult.hypothesis.deviations.total' AND {}{} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t),
        scope::and(scope::table_predicate("metric_sums", scopes))
    )
}

fn sql_mttr_value(f: i64, t: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS v FROM run_roots AS s WHERE {}",
        tumult_lake::mean_recovery_time_sql("s"),
        roots_in(f, t, scopes)
    )
}

fn sql_mttr_spark(f: i64, t: i64, b: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS ts, {} AS v FROM run_roots AS s WHERE {} GROUP BY 1 ORDER BY 1",
        bucket_of("s.ts_ns", b),
        tumult_lake::mean_recovery_time_sql("s"),
        roots_in(f, t, scopes)
    )
}

fn sql_coverage_value(f: i64, t: i64, scopes: &[String]) -> String {
    format!(
        "SELECT COUNT(DISTINCT target_system) AS v FROM spans WHERE target_system IS NOT NULL AND {}{}",
        w(f, t),
        scope::and(env_trace_exists("spans", scopes))
    )
}

fn sql_coverage_spark(f: i64, t: i64, b: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS ts, COUNT(DISTINCT target_system) AS v FROM spans \
         WHERE target_system IS NOT NULL AND {}{} GROUP BY 1 ORDER BY 1",
        bucket_expr(b),
        w(f, t),
        scope::and(env_trace_exists("spans", scopes))
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
        label: "Run pass rate",
        unit: "ratio",
        value: sql_pass_rate_value,
        spark_num: sql_pass_rate_spark,
        spark_den: None,
    },
    Kpi {
        name: "deviation_rate",
        label: "Deviation rate",
        unit: "ratio",
        value: sql_deviation_value,
        spark_num: sql_deviation_num,
        spark_den: Some(sql_traced_runs_spark),
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
) -> Result<Json<Value>, ApiError> {
    let range = params.range.unwrap_or_else(|| "24h".into());
    let Some((cur, prev)) = windows(&range) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": invalid_range(&range)})),
        )
            .into_response()
            .into());
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

        let per_day = series(reader, &sql_per_day(cur.0, cur.1, &scopes))?;

        // Every run of a target, and its run pass rate as the card counts it.
        let targets = reader
            .query_json_rows(&sql_targets(cur.0, cur.1, &scopes))
            .map_err(|e| e.to_string())?;

        let faults = reader
            .query_json_rows(&sql_faults(cur.0, cur.1, &scopes))
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

/// The runs per day in `[f, t)` in the scopes.
fn sql_per_day(f: i64, t: i64, scopes: &[String]) -> String {
    format!(
        "SELECT {} AS ts, COUNT(*) AS v FROM run_roots WHERE {}{} \
         GROUP BY 1 ORDER BY 1",
        bucket_expr(86_400),
        w(f, t),
        and_root_env(scopes)
    )
}

/// The ten targets with the most runs in `[f, t)` in the scopes, with the
/// run pass rate of each.
fn sql_targets(f: i64, t: i64, scopes: &[String]) -> String {
    format!(
        "SELECT s.target_system AS target, count(*) AS experiments, \
         {} AS pass_rate {} AND s.target_system IS NOT NULL \
         GROUP BY 1 ORDER BY experiments DESC LIMIT 10",
        run_pass_rate_sql("s", "l"),
        runs_from(f, t, scopes)
    )
}

/// The statements of the overview, for the SQL corpus: the value and the
/// sparkline of each card, in every scope, then the lists of the page.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    use tumult_test_utils::sql_corpus::Statement;
    let (f, t) = (1_774_900_000_000_000_000, 1_774_986_400_000_000_000);
    let mut out = Vec::new();
    for (label, scopes) in [("all", Vec::new()), ("prod", vec!["prod".to_string()])] {
        for kpi in KPIS {
            let name = |part: &str| format!("{}:KPIS[{}].{part} in {label}", file!(), kpi.name);
            out.push(Statement::new(name("value"), (kpi.value)(f, t, &scopes)));
            out.push(Statement::new(
                name("spark_num"),
                (kpi.spark_num)(f, t, 3_600, &scopes),
            ));
            if let Some(den) = kpi.spark_den {
                out.push(Statement::new(name("spark_den"), den(f, t, 3_600, &scopes)));
            }
        }
    }
    let prod = vec!["prod".to_string()];
    out.extend(tumult_test_utils::sql_statements![
        sql_per_day(f, t, &prod),
        sql_targets(f, t, &prod),
        sql_faults(f, t, &[]),
        sql_faults(f, t, &prod),
    ]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tumult_lake::Store;
    use tumult_test_utils::recorded;

    #[test]
    fn the_runs_of_a_window_read_the_completion_logs_since_its_start() {
        let hour = 3_600 * 1_000_000_000_i64;
        let q = runs_from(2 * hour, 3 * hour, &[]);
        assert!(
            q.contains(&tumult_lake::run_completions_sql(Some(2 * hour))),
            "the completion log join is bounded: {q}"
        );
    }

    /// The recording is what the runner sent for real runs: 78 probe
    /// samples, 4 actions of the methods (2 in prod, 2 in staging) and 2
    /// steps of rollbacks, every step served by the process plugin.
    #[test]
    fn the_fault_pie_of_the_recorded_runs_counts_the_actions_of_their_methods() {
        let dir = tempfile::tempdir().expect("a directory");
        let store = Store::open(&dir.path().join("lake.duckdb")).expect("a store");
        let journals = recorded::load(&store.writer().expect("a writer"));
        let reader = store.read_only().expect("a reader");
        let end = recorded::day_after(&journals);
        let pie = |scopes: &[&str]| {
            let scopes: Vec<String> = scopes.iter().map(ToString::to_string).collect();
            reader
                .query_json_rows(&sql_faults(0, end, &scopes))
                .expect("the query")
        };
        let injections = |count: u64| {
            vec![json!({"fault_type": "injection", "fault_subtype": "process", "count": count})]
        };

        assert_eq!(pie(&[]), injections(4));
        assert_eq!(pie(&["prod"]), injections(2));
        assert_eq!(pie(&["staging"]), injections(2));
        assert_eq!(pie(&["lab"]), Vec::<Value>::new());
    }
}
