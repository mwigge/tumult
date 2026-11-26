//! `GET /api/experiments` (+ `/windows`, `/{id}`) and `GET /api/dimensions`.

use crate::error::ApiError;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_lake::{scope, Reader, SqlParam};

use crate::auth::Principal;
use crate::sql_util::{
    env_trace_exists, invalid_range, run_outcome_where, sql_contains, sql_string, windows,
    with_reader,
};
use crate::ApiState;

// ---------------------------------------------------------------------------
// GET /api/experiments + /api/experiments/{id}

#[derive(Deserialize)]
pub(crate) struct ExperimentParams {
    range: Option<String>,
    outcome: Option<String>,
    target: Option<String>,
    fault: Option<String>,
    q: Option<String>,
    /// The whole experiment name, compared exactly.
    name: Option<String>,
    origin: Option<String>,
}

/// Longest `q` or `name` accepted, in characters.
const MAX_TEXT_FILTER: usize = 200;

/// `400` when the text filter `field` is longer than [`MAX_TEXT_FILTER`].
fn short_enough(field: &str, value: &str) -> Result<(), ApiError> {
    if value.chars().count() <= MAX_TEXT_FILTER {
        return Ok(());
    }
    Err(crate::error::bad_request(format!(
        "{field} too long (max {MAX_TEXT_FILTER} chars)"
    )))
}

/// The run-root ↔ completed-log join used by the list and detail queries.
/// `run_roots` holds the root span of a run, or a root built from its
/// journal when the run sent no trace, and `run_completions` the last
/// completion log of each run.
const EXPERIMENT_FROM: &str =
    "FROM run_roots s LEFT JOIN run_completions l ON s.experiment_id = l.experiment_id";

/// The columns of one run over [`EXPERIMENT_FROM`].
fn experiment_cols() -> String {
    format!(
        "s.experiment_id AS id, s.experiment_name AS name, \
         s.ts_ns AS started_ns, s.duration_ns, s.trace_id, \
         s.target_system, s.target_technology, s.target_environment, \
         {} AS status, \
         l.log_attrs['deviations'] AS deviations, \
         l.log_attrs['duration_ms'] AS duration_ms, \
         s.journal_only",
        tumult_lake::run_outcome_sql("s", "l")
    )
}

pub(crate) async fn experiments(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<ExperimentParams>,
) -> Result<Json<Value>, ApiError> {
    let bad = |msg: String| -> ApiError {
        (StatusCode::BAD_REQUEST, Json(json!({"error": msg})))
            .into_response()
            .into()
    };
    let mut wheres = vec!["s.span_name = 'resilience.experiment'".to_string()];
    // Per-user environment scoping (empty scopes = all environments).
    if let Some(env) = scope::in_list("s.target_environment", &principal.env_scopes) {
        wheres.push(env);
    }
    if let Some(range) = &params.range {
        let Some((cur, _)) = windows(range) else {
            return Err(bad(invalid_range(range)));
        };
        wheres.push(window_where_sql("s.ts_ns", cur));
    }
    if let Some(outcome) = &params.outcome {
        wheres.push(run_outcome_where(outcome, "s", "l").map_err(bad)?);
    }
    if let Some(target) = &params.target {
        wheres.push(equals_sql("s.target_system", target));
    }
    if let Some(fault) = &params.fault {
        // Child spans (actions/probes) carry the fault but not experiment_id —
        // they correlate with the run through trace_id.
        wheres.push(fault_where_sql(fault));
    }
    if let Some(q) = &params.q {
        short_enough("q", q)?;
        wheres.push(like_sql("s.experiment_name", "s.experiment_id", q));
    }
    if let Some(name) = &params.name {
        if name.is_empty() {
            return Err(bad("name must not be empty".into()));
        }
        short_enough("name", name)?;
        wheres.push(equals_sql("s.experiment_name", name));
    }
    let origin = params.origin.as_deref();
    if let Some(o) = origin {
        if o != "automated" && o != "manual" {
            return Err(bad(format!(
                "invalid origin {o:?}; expected automated|manual"
            )));
        }
    }
    // Manual records that are not drafts join the view unless explicitly
    // excluded; they have no tumult outcome/fault taxonomy, so those two
    // filters drop the manual branch entirely (documented behaviour).
    let include_manual =
        origin != Some("automated") && params.outcome.is_none() && params.fault.is_none();
    let include_spans = origin != Some("manual");

    let spans_sql = spans_list_sql(&wheres);

    let sql = if !include_manual {
        spans_only_sql(&spans_sql)
    } else {
        // The manual branch mirrors the span columns; range/target/q filters
        // map onto the manual columns. A draft is still being written: it
        // is no test yet, so it is not on the list.
        let mut mwheres = vec!["m.status <> 'draft'".to_string()];
        if let Some(env) = scope::in_list("m.target_environment", &principal.env_scopes) {
            mwheres.push(env);
        }
        if let Some(range) = &params.range {
            let Some((cur, _)) = windows(range) else {
                return Err(bad(invalid_range(range)));
            };
            mwheres.push(window_where_sql("m.executed_at_ns", cur));
        }
        if let Some(target) = &params.target {
            mwheres.push(equals_sql("m.target_system", target));
        }
        if let Some(q) = &params.q {
            mwheres.push(like_sql("m.experiment_name", "m.id", q));
        }
        if let Some(name) = &params.name {
            mwheres.push(equals_sql("m.experiment_name", name));
        }
        let manual_sql = manual_list_sql(&mwheres);
        if include_spans {
            both_sql(&spans_sql, &manual_sql)
        } else {
            manual_only_sql(&manual_sql)
        }
    };
    let rows = with_reader(&state.db_path, move |reader| {
        reader.query_json_rows(&sql).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "experiments": rows})))
}

#[derive(Deserialize)]
pub(crate) struct ExperimentWindowsParams {
    from: Option<i64>,
    to: Option<i64>,
}

/// `GET /api/experiments/windows?from=&to=` — experiment runs overlapping
/// the `[from, to)` window (epoch ns), for chart overlays. Cheaper than the
/// list endpoint: one row per run from the `experiment_runs` rollup view.
pub(crate) async fn experiment_windows(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<ExperimentWindowsParams>,
) -> Result<Json<Value>, ApiError> {
    let (Some(from), Some(to)) = (params.from, params.to) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "missing query parameters: from and to (epoch ns)"})),
        )
            .into_response()
            .into());
    };
    if from >= to {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "from must be before to"})),
        )
            .into_response()
            .into());
    }
    let sql = run_windows_sql(from, to, &principal.env_scopes);
    let rows = with_reader(&state.db_path, move |reader| {
        reader.query_json_rows(&sql).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "runs": rows})))
}

/// How long after the end of a run a metric point of it may be stamped:
/// one export interval of the metrics reader.
const METRIC_EXPORT_GRACE_NS: i64 = 60_000_000_000;

/// The metric points that carry the name of the run `info` and are of its
/// time. Every run of an experiment has the same name, so the time is
/// what tells the points of one run from those of the next.
fn metric_points(reader: &Reader, info: &Value) -> Result<Vec<Value>, String> {
    let name = info.get("name").and_then(Value::as_str);
    let started_ns = info.get("started_ns").and_then(Value::as_i64);
    let (Some(name), Some(started_ns)) = (name, started_ns) else {
        return Ok(Vec::new());
    };
    let duration_ns = info.get("duration_ns").and_then(Value::as_i64);
    let last_ns = started_ns
        .saturating_add(duration_ns.unwrap_or(0))
        .saturating_add(METRIC_EXPORT_GRACE_NS);
    reader
        .query_json_rows_bound(
            crate::sql::METRIC_POINTS,
            &[
                SqlParam::Text(name.to_string()),
                SqlParam::Int(started_ns),
                SqlParam::Int(last_ns),
            ],
        )
        .map_err(|e| e.to_string())
}

pub(crate) async fn experiment_detail(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    if id.chars().count() > 100 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "experiment id too long"})),
        )
            .into_response()
            .into());
    }
    // Environments outside the principal's scopes look exactly like a
    // missing experiment (404 — no existence leak across scopes).
    let env_where = scope::in_list("s.target_environment", &principal.env_scopes)
        .map_or(String::new(), |e| format!(" AND {e}"));
    let body = with_reader(&state.db_path, move |reader| {
        let id_sql = sql_string(&id);
        let info = reader
            .query_json_rows(&detail_sql(&id_sql, &env_where))
            .map_err(|e| e.to_string())?;
        let Some(info) = info.into_iter().next() else {
            return Ok(None);
        };

        // Tumult sets experiment_id only on the root span; the rest of the
        // run's span tree (probes, actions, rollbacks) shares its trace_id.
        let spans = reader
            .query_json_rows(&detail_spans_sql(&id_sql))
            .map_err(|e| e.to_string())?;

        let trace_ids: Vec<String> = spans
            .iter()
            .filter_map(|s| s.get("trace_id").and_then(Value::as_str))
            .map(sql_string)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let logs = reader
            .query_json_rows(&detail_logs_sql(&id_sql, &trace_ids))
            .map_err(|e| e.to_string())?;

        let metrics = metric_points(reader, &info)?;
        let evidence = super::evidence::of_run(reader, &id)?;

        Ok(Some(json!({
            "experiment": info,
            "spans": spans,
            "logs": logs,
            "metrics": metrics,
            "evidence": evidence,
        })))
    })
    .await?;
    match body {
        Some(body) => Ok(Json(body)),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "experiment not found"})),
        )
            .into_response()
            .into()),
    }
}

// ---------------------------------------------------------------------------
// GET /api/dimensions
pub(crate) async fn dimensions(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, ApiError> {
    let scopes = principal.env_scopes.clone();
    let body = with_reader(&state.db_path, move |reader| {
        // outcomes/targets/experiments bind the environment column of the
        // row itself; faults (child spans) reach it by correlation. A run
        // that sent no trace names its target only on its run root.
        let distinct = |sql: &str| -> Result<Vec<Value>, String> {
            let rows = reader.query_json_rows(sql).map_err(|e| e.to_string())?;
            Ok(rows
                .into_iter()
                .filter_map(|r| r.get("v").cloned())
                .collect())
        };
        Ok(json!({
            "outcomes": distinct(&outcomes_sql(&scopes))?,
            "targets": distinct(&targets_sql(&scopes))?,
            "faults": distinct(&faults_sql(&scopes))?,
            "experiments": distinct(&names_sql(&scopes))?,
        }))
    })
    .await?;
    Ok(Json(body))
}

/// The runs whose child spans injected the fault `fault`, as a predicate
/// on a run root `s`. Child spans (actions/probes) carry the fault but not
/// experiment_id; they correlate with the run through trace_id.
fn fault_where_sql(fault: &str) -> String {
    format!(
        "EXISTS (SELECT 1 FROM spans c WHERE c.trace_id = s.trace_id \
         AND c.fault_type = {})",
        sql_string(fault)
    )
}

/// The rows whose `column` falls in the window `cur`, `[from, to)`.
fn window_where_sql(column: &str, cur: (i64, i64)) -> String {
    format!("{column} >= {} AND {column} < {}", cur.0, cur.1)
}

/// The rows whose `column` is the text `value`.
fn equals_sql(column: &str, value: &str) -> String {
    format!("{column} = {}", sql_string(value))
}

/// The rows whose column `name` or `id` holds the text `q`.
fn like_sql(name: &str, id: &str, q: &str) -> String {
    let like = sql_contains(q);
    format!(
        "({name} ILIKE {like} ESCAPE '\\' \
         OR {id} ILIKE {like} ESCAPE '\\')"
    )
}

/// The run roots that `wheres` keeps, as rows of the list.
fn spans_list_sql(wheres: &[String]) -> String {
    format!(
        "SELECT {}, \
         (SELECT string_agg(DISTINCT c.fault_type, ',') FROM spans c \
          WHERE c.trace_id = s.trace_id AND c.fault_type IS NOT NULL) AS faults, \
         'automated' AS origin, NULL AS review_status \
         {EXPERIMENT_FROM} WHERE {}",
        experiment_cols(),
        wheres.join(" AND ")
    )
}

/// The manual records that `mwheres` keeps, as rows of the list.
fn manual_list_sql(mwheres: &[String]) -> String {
    format!(
        "SELECT m.id, m.experiment_name AS name, m.executed_at_ns AS started_ns, \
         CAST(m.duration_s * 1000000000 AS BIGINT) AS duration_ns, \
         NULL AS trace_id, m.target_system, NULL AS target_technology, \
         m.target_environment, m.outcome_status AS status, \
         NULL AS deviations, NULL AS duration_ms, false AS journal_only, \
         NULL AS faults, \
         'manual' AS origin, m.status AS review_status \
         FROM manual_experiments m WHERE {}",
        mwheres.join(" AND ")
    )
}

/// The newest 500 rows of [`spans_list_sql`] alone.
fn spans_only_sql(spans_sql: &str) -> String {
    format!("{spans_sql} ORDER BY s.ts_ns DESC LIMIT 500")
}

/// The newest 500 rows of both branches of the list.
fn both_sql(spans_sql: &str, manual_sql: &str) -> String {
    format!(
        "SELECT * FROM ({spans_sql} UNION ALL {manual_sql}) \
         ORDER BY started_ns DESC LIMIT 500"
    )
}

/// The newest 500 rows of [`manual_list_sql`] alone.
fn manual_only_sql(manual_sql: &str) -> String {
    format!("{manual_sql} ORDER BY started_ns DESC LIMIT 500")
}

/// The runs overlapping `[from, to)` in the environments `scopes`. The
/// rollup view has no environment column; scope binds through the
/// `target_environment` of the run's root by experiment_id.
fn run_windows_sql(from: i64, to: i64, scopes: &[String]) -> String {
    let env = scope::in_list("se.target_environment", scopes).map_or_else(String::new, |env| {
        format!(
            " AND EXISTS (SELECT 1 FROM run_roots se \
             WHERE se.experiment_id = experiment_runs.experiment_id AND {env})"
        )
    });
    format!(
        "SELECT experiment_id AS id, experiment_name AS name, \
         started_at_ns AS start_ns, ended_at_ns AS end_ns, outcome_status AS outcome \
         FROM experiment_runs \
         WHERE started_at_ns < {to} AND ended_at_ns > {from}{env} \
         ORDER BY started_at_ns LIMIT 200"
    )
}

/// The run `id_sql`, quoted, that the scope `env_where` keeps.
fn detail_sql(id_sql: &str, env_where: &str) -> String {
    format!(
        "SELECT {} {EXPERIMENT_FROM} \
         WHERE s.experiment_id = {id_sql}{env_where}",
        experiment_cols()
    )
}

/// The span tree of the run `id_sql`, quoted. Tumult sets experiment_id
/// only on the root span; the rest of the run's span tree (probes,
/// actions, rollbacks) shares its trace_id.
fn detail_spans_sql(id_sql: &str) -> String {
    format!(
        "SELECT ts_ns, trace_id, span_id, parent_span_id, span_name, span_kind, \
         duration_ns, status_code, status_message, service_name, \
         fault_type, fault_subtype, span_attrs, events \
         FROM spans \
         WHERE experiment_id = {id_sql} \
            OR trace_id IN (SELECT trace_id FROM spans WHERE experiment_id = {id_sql}) \
         ORDER BY ts_ns LIMIT 2000"
    )
}

/// The logs of the run `id_sql` and of its traces `trace_ids`, all quoted.
fn detail_logs_sql(id_sql: &str, trace_ids: &[String]) -> String {
    let mut log_where = format!("log_attrs['experiment_id'] = {id_sql}");
    if !trace_ids.is_empty() {
        log_where.push_str(&format!(" OR trace_id IN ({})", trace_ids.join(", ")));
    }
    format!(
        "SELECT ts_ns, severity_text, body, trace_id, span_id, log_attrs \
         FROM logs WHERE {log_where} ORDER BY ts_ns LIMIT 1000"
    )
}

/// The outcomes of the runs in the environments `scopes`. A run that sent
/// no trace names its target only on its run root.
fn outcomes_sql(scopes: &[String]) -> String {
    let and_run = scope::and(scope::in_list("s.target_environment", scopes));
    let status = tumult_lake::run_outcome_sql("s", "l");
    format!(
        "SELECT DISTINCT {status} AS v {EXPERIMENT_FROM} \
         WHERE {status} IS NOT NULL{and_run} \
         ORDER BY 1"
    )
}

/// The targets of the runs in the environments `scopes`.
fn targets_sql(scopes: &[String]) -> String {
    let and_root = scope::and(scope::in_list("target_environment", scopes));
    format!(
        "SELECT DISTINCT target_system AS v FROM ( \
         SELECT target_system, target_environment FROM spans \
         UNION ALL \
         SELECT target_system, target_environment FROM run_roots WHERE journal_only) \
         WHERE target_system IS NOT NULL{and_root} ORDER BY 1"
    )
}

/// The faults of the runs in the environments `scopes`: child spans reach
/// the environment by correlation.
fn faults_sql(scopes: &[String]) -> String {
    let and_trace = scope::and(env_trace_exists("spans", scopes));
    format!(
        "SELECT DISTINCT fault_type AS v FROM spans \
         WHERE fault_type IS NOT NULL{and_trace} ORDER BY 1"
    )
}

/// The names of the experiments in the environments `scopes`.
fn names_sql(scopes: &[String]) -> String {
    let and_root = scope::and(scope::in_list("target_environment", scopes));
    format!(
        "SELECT DISTINCT experiment_name AS v FROM run_roots \
         WHERE experiment_name IS NOT NULL{and_root} ORDER BY 1"
    )
}

/// The statements of the experiments, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    let prod = vec!["prod".to_string()];
    let cur = (1_774_900_000_000_000_000, 1_774_986_400_000_000_000);
    let wheres = vec![
        "s.span_name = 'resilience.experiment'".to_string(),
        "s.target_environment IN ('prod')".to_string(),
        window_where_sql("s.ts_ns", cur),
        run_outcome_where("failed", "s", "l").expect("an outcome"),
        equals_sql("s.target_system", "payments"),
        fault_where_sql("network.latency"),
        like_sql("s.experiment_name", "s.experiment_id", "checkout"),
        equals_sql("s.experiment_name", "checkout-latency"),
    ];
    let mwheres = vec![
        "m.status <> 'draft'".to_string(),
        window_where_sql("m.executed_at_ns", cur),
        equals_sql("m.target_system", "payments"),
        like_sql("m.experiment_name", "m.id", "restore"),
        equals_sql("m.experiment_name", "restore-drill"),
    ];
    let spans = spans_list_sql(&wheres);
    let manual = manual_list_sql(&mwheres);
    let id_sql = sql_string("run-1");
    tumult_test_utils::sql_statements![
        spans_only_sql(&spans),
        both_sql(&spans, &manual),
        manual_only_sql(&manual),
        run_windows_sql(1_774_900_000_000_000_000, 1_774_986_400_000_000_000, &[]),
        run_windows_sql(1_774_900_000_000_000_000, 1_774_986_400_000_000_000, &prod),
        detail_sql(&id_sql, ""),
        detail_sql(&id_sql, " AND s.target_environment IN ('prod')"),
        detail_spans_sql(&id_sql),
        detail_logs_sql(&id_sql, &[]),
        detail_logs_sql(&id_sql, &[sql_string("trace-1"), sql_string("trace-2")]),
        outcomes_sql(&prod),
        targets_sql(&prod),
        faults_sql(&prod),
        names_sql(&[]),
    ]
}
