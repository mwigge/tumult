//! `GET /api/experiments` (+ `/windows`, `/{id}`) and `GET /api/dimensions`.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::sql_util::{
    and_pred, env_scope_where, env_trace_exists, sql_contains, sql_string, windows, with_reader,
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

pub(crate) async fn experiments(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<ExperimentParams>,
) -> Result<Json<Value>, Response> {
    let bad = |msg: String| (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response();
    let mut wheres = vec!["s.span_name = 'resilience.experiment'".to_string()];
    // Per-user environment scoping (empty scopes = all environments).
    if let Some(env) = env_scope_where("s.target_environment", &principal.env_scopes) {
        wheres.push(env);
    }
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
        if let Some(env) = env_scope_where("m.target_environment", &principal.env_scopes) {
            mwheres.push(env);
        }
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
    // The rollup view has no environment column; scope binds through the
    // root span's `target_environment` by experiment_id.
    let env = env_scope_where("se.target_environment", &principal.env_scopes).map_or_else(
        String::new,
        |env| {
            format!(
                " AND EXISTS (SELECT 1 FROM spans se \
                 WHERE se.experiment_id = experiment_runs.experiment_id AND {env})"
            )
        },
    );
    let sql = format!(
        "SELECT experiment_id AS id, experiment_name AS name, \
         started_at_ns AS start_ns, ended_at_ns AS end_ns, outcome_status AS outcome \
         FROM experiment_runs \
         WHERE started_at_ns < {to} AND ended_at_ns > {from}{env} \
         ORDER BY started_at_ns LIMIT 200"
    );
    let rows = with_reader(&state.db_path, move |reader| {
        reader.query_json_rows(&sql).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "runs": rows})))
}

pub(crate) async fn experiment_detail(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    if id.chars().count() > 100 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "experiment id too long"})),
        )
            .into_response());
    }
    // Environments outside the principal's scopes look exactly like a
    // missing experiment (404 — no existence leak across scopes).
    let env_where = env_scope_where("s.target_environment", &principal.env_scopes)
        .map_or(String::new(), |e| format!(" AND {e}"));
    let body = with_reader(&state.db_path, move |reader| {
        let id_sql = sql_string(&id);
        let info = reader
            .query_json_rows(&format!(
                "SELECT {EXPERIMENT_COLS} {EXPERIMENT_FROM} \
                 WHERE s.span_name = 'resilience.experiment' AND s.experiment_id = {id_sql}{env_where}"
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
// GET /api/dimensions
pub(crate) async fn dimensions(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, Response> {
    let scopes = principal.env_scopes.clone();
    let body = with_reader(&state.db_path, move |reader| {
        // targets/experiments bind the root span's own environment column;
        // outcomes (logs) and faults (child spans) reach it by correlation.
        let and_root = and_pred(env_scope_where("target_environment", &scopes));
        let and_trace = and_pred(env_trace_exists("spans", &scopes));
        let and_logs = and_pred(
            env_scope_where("se.target_environment", &scopes).map(|env| {
                format!(
                    "EXISTS (SELECT 1 FROM spans se \
                 WHERE se.experiment_id = logs.log_attrs['experiment_id'] AND {env})"
                )
            }),
        );
        let distinct = |sql: &str| -> Result<Vec<Value>, String> {
            let rows = reader.query_json_rows(sql).map_err(|e| e.to_string())?;
            Ok(rows
                .into_iter()
                .filter_map(|r| r.get("v").cloned())
                .collect())
        };
        Ok(json!({
            "outcomes": distinct(&format!(
                "SELECT DISTINCT log_attrs['status'] AS v FROM logs \
                 WHERE body = 'experiment.completed' AND log_attrs['status'] IS NOT NULL{and_logs} \
                 ORDER BY 1"))?,
            "targets": distinct(&format!(
                "SELECT DISTINCT target_system AS v FROM spans \
                 WHERE target_system IS NOT NULL{and_root} ORDER BY 1"))?,
            "faults": distinct(&format!(
                "SELECT DISTINCT fault_type AS v FROM spans \
                 WHERE fault_type IS NOT NULL{and_trace} ORDER BY 1"))?,
            "experiments": distinct(&format!(
                "SELECT DISTINCT experiment_name AS v FROM spans \
                 WHERE span_name = 'resilience.experiment' \
                   AND experiment_name IS NOT NULL{and_root} ORDER BY 1"))?,
        }))
    })
    .await?;
    Ok(Json(body))
}
