//! Run reads: the list (`GET /api/runs`), one run with its audit trail and
//! approval chain (`GET /api/runs/{id}`), and the audit hash-chain check
//! (`GET /api/runs/{id}/audit/verify`).

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_lake::run_state;

use crate::auth::Principal;
use crate::error::{bad_request, not_found};
use crate::sql_util::{sql_string, with_reader};
use crate::ApiState;

/// Every valid `runs.state` value (active + terminal), for `?state=`.
const STATES: &[&str] = &[
    run_state::QUEUED,
    run_state::VALIDATING,
    run_state::RUNNING,
    run_state::STOPPING,
    run_state::PASSED,
    run_state::DEVIATED,
    run_state::FAILED,
    run_state::ABORTED,
    run_state::ORPHANED,
    run_state::ROLLBACK_PENDING,
    run_state::PENDING_APPROVAL,
    run_state::REJECTED,
    run_state::EXPIRED,
];

#[derive(Debug, Deserialize)]
pub struct ListParams {
    state: Option<String>,
    limit: Option<u32>,
    /// Campaign children of one parent run (schema v12 `runs.gameday_id`).
    gameday_id: Option<String>,
}

/// `GET /api/runs?state=&limit=` — runs, newest first (limit defaults to
/// 100, capped at 500). Runs whose experiment's environment is outside the
/// principal's scopes are hidden; runs without an experiment yet (still
/// queued) stay visible to everyone — the environment is known only once
/// execution links the journal's `experiment_id` (documented behaviour).
pub async fn list(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, Response> {
    if let Some(state) = params.state.as_deref().filter(|s| !s.is_empty()) {
        if !STATES.contains(&state) {
            return Err(bad_request(format!(
                "invalid state {state:?}; expected one of {}",
                STATES.join(", ")
            )));
        }
    }
    let limit = params.limit.unwrap_or(100).clamp(1, 500);
    let state_filter = params.state.filter(|s| !s.is_empty());
    let gameday_filter = params.gameday_id.filter(|s| !s.is_empty());
    if gameday_filter
        .as_deref()
        .is_some_and(|id| id.chars().count() > 100)
    {
        return Err(bad_request("gameday id too long"));
    }
    let scopes = principal.env_scopes.clone();
    let rows = with_reader(&state.db_path, move |reader| {
        let state_clause = state_filter.as_deref().map_or(String::new(), |s| {
            format!("AND r.state = {}", sql_string(s))
        });
        let gameday_clause = gameday_filter.as_deref().map_or(String::new(), |g| {
            format!("AND r.gameday_id = {}", sql_string(g))
        });
        if scopes.is_empty() && gameday_clause.is_empty() {
            return reader
                .runs(state_filter.as_deref(), limit)
                .map_err(|e| e.to_string());
        }
        // The `experiments` analytics table has no env column; spans do.
        let scope_clause = if scopes.is_empty() {
            String::from("TRUE")
        } else {
            let env_list = scopes
                .iter()
                .map(|s| sql_string(s))
                .collect::<Vec<_>>()
                .join(", ");
            format!("(e.env IN ({env_list}) OR r.experiment_id IS NULL)")
        };
        reader
            .query_json_rows(&format!(
                "SELECT r.*, g.name AS definition_name FROM runs r \
                 LEFT JOIN run_registry g ON g.id = r.registry_id \
                 LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env \
                            FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id \
                 WHERE {scope_clause} {state_clause} {gameday_clause} \
                 ORDER BY r.queued_at_ns DESC LIMIT {limit}"
            ))
            .map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "runs": rows})))
}

/// `GET /api/runs/{id}` — one run plus its audit trail (oldest first) and
/// its approval chain: `approval.request` (the pinned request, `null` for
/// T0 runs that never gate) and `approval.decisions` (oldest first).
/// Runs in an environment outside the principal's scopes 404 (same rule as
/// the list; runs without an experiment stay visible).
pub async fn detail(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    if id.chars().count() > 100 {
        return Err(bad_request("run id too long"));
    }
    let scopes = principal.env_scopes.clone();
    let lookup = id.clone();
    let body = with_reader(&state.db_path, move |reader| {
        let run = if scopes.is_empty() {
            reader.run_get(&lookup).map_err(|e| e.to_string())?
        } else {
            let env_list = scopes
                .iter()
                .map(|s| sql_string(s))
                .collect::<Vec<_>>()
                .join(", ");
            reader
                .query_json_rows(&format!(
                    "SELECT r.*, g.name AS definition_name FROM runs r \
                     LEFT JOIN run_registry g ON g.id = r.registry_id \
                     LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env \
                                FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id \
                     WHERE r.id = {} \
                       AND (e.env IN ({env_list}) OR r.experiment_id IS NULL)",
                    sql_string(&lookup)
                ))
                .map_err(|e| e.to_string())?
                .into_iter()
                .next()
        };
        let audit = reader.run_audit_trail(&lookup).map_err(|e| e.to_string())?;
        let request = reader
            .approval_request(&lookup)
            .map_err(|e| e.to_string())?;
        let decisions = reader
            .approval_decisions(&lookup)
            .map_err(|e| e.to_string())?;
        Ok(run.map(|run| {
            json!({
                "run": run,
                "audit": audit,
                "approval": {"request": request, "decisions": decisions},
            })
        }))
    })
    .await?;
    match body {
        Some(body) => Ok(Json(body)),
        None => Err(not_found(format!("unknown run id {id:?}"))),
    }
}

/// `GET /api/runs/{id}/audit/verify` — re-verify the run's audit hash chain
/// (schema v7): every chained row's stored hash is recomputed and the
/// `prev_hash` pointers are checked pairwise (tumult-lake's
/// `Reader::verify_run_audit_chain`).
/// `chain_valid: false` means the trail was tampered with. 404 for unknown
/// runs and for runs outside the principal's environment scopes (same rule
/// as [`detail`]); `detail` already returns the trail itself, so this
/// exposes nothing new.
pub async fn audit_verify(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    if id.chars().count() > 100 {
        return Err(bad_request("run id too long"));
    }
    let scopes = principal.env_scopes.clone();
    let lookup = id.clone();
    let result = with_reader(&state.db_path, move |reader| {
        let exists = if scopes.is_empty() {
            reader
                .run_get(&lookup)
                .map_err(|e| e.to_string())?
                .is_some()
        } else {
            let env_list = scopes
                .iter()
                .map(|s| sql_string(s))
                .collect::<Vec<_>>()
                .join(", ");
            !reader
                .query_json_rows(&format!(
                    "SELECT r.id FROM runs r \
                     LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env \
                                FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id \
                     WHERE r.id = {} \
                       AND (e.env IN ({env_list}) OR r.experiment_id IS NULL)",
                    sql_string(&lookup)
                ))
                .map_err(|e| e.to_string())?
                .is_empty()
        };
        if !exists {
            return Ok(None);
        }
        reader
            .verify_run_audit_chain(&lookup)
            .map(Some)
            .map_err(|e| e.to_string())
    })
    .await?;
    match result {
        Some(chain_valid) => Ok(Json(json!({
            "run_id": id,
            "chain_valid": chain_valid,
        }))),
        None => Err(not_found(format!("unknown run id {id:?}"))),
    }
}
