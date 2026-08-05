//! Run control: enqueue (`POST /api/runs`), e-stop (`POST /api/runs/{id}/stop`)
//! and the global halt (`POST /api/runs/stop-all`).

use std::collections::HashMap;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_ingest::{EnqueueError, RunRequest, StopError};
use tumult_lake::run_state;

use crate::auth::Principal;
use crate::error::{bad_request, forbidden, not_found, unavailable};
use crate::sql_util::{internal, sql_string, with_reader};
use crate::ApiState;

/// JSON body: which registered definition to run, plus template variables
/// and the approval-relevant execution context (`env` defaults to `"dev"`).
#[derive(Debug, Deserialize)]
pub struct CreateRunRequest {
    registry_id: String,
    #[serde(default)]
    vars: HashMap<String, String>,
    #[serde(default = "default_env")]
    env: String,
    #[serde(default)]
    target: Option<String>,
}

fn default_env() -> String {
    "dev".into()
}

/// `POST /api/runs` — classify the definition into a risk tier (T0–T3,
/// [`tumult_ingest::approvals::classify`], ADR-013) at request time. T0
/// enqueues directly onto the daemon's bounded run queue: 202 with the run
/// id, 429 when the waiting queue is at capacity (backpressure, never
/// silent unbounded queueing). T1–T3 park in `pending_approval` (202 with
/// the tier) until the approval quorum dispatches them — see
/// [`crate::approvals`]. The definition is re-validated here, so an invalid
/// definition now fails with 400 at request time instead of failing the run
/// at dispatch. The `enqueued`/`requested` audit event records the
/// authenticated principal as actor. A scoped principal may only launch into
/// its own environments: any other `env` is a 403.
pub async fn create(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<CreateRunRequest>,
) -> Result<Response, Response> {
    let Some(queue) = state.runs_handle() else {
        return Err(unavailable("run queue is not wired"));
    };
    // A scoped principal may only launch into its own environments (same
    // rule as the run reads); checked before anything else resolves.
    if !principal.env_allowed(&req.env) {
        return Err(forbidden(format!(
            "environment {:?} is outside the principal's scopes",
            req.env
        )));
    }
    let def = super::registry_or_404(&state, &req.registry_id).await?;
    let (experiment, _env) =
        tumult_ingest::prepare_run(&def.definition_toon, &req.vars).map_err(bad_request)?;
    let introspection = tumult_ingest::approvals::introspect(&experiment);
    let tier = tumult_ingest::approvals::classify(&tumult_ingest::approvals::TierInput {
        env: req.env.clone(),
        // No T0 pre-approved catalog is configured yet.
        catalog_matched: false,
        introspection,
    });
    let request = RunRequest {
        registry_id: def.id,
        definition_toon: def.definition_toon,
        vars: req.vars,
        env: req.env,
        target: req.target,
    };
    if tier != tumult_ingest::approvals::Tier::T0 {
        return match queue.request_gated(request, tier, principal.actor()).await {
            Ok(run_id) => Ok((
                StatusCode::ACCEPTED,
                Json(json!({
                    "run_id": run_id,
                    "state": run_state::PENDING_APPROVAL,
                    "tier": tier.as_str(),
                })),
            )
                .into_response()),
            Err(EnqueueError::Full) => Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(json!({"error": "run queue full; retry later"})),
            )
                .into_response()),
            Err(EnqueueError::Store(e)) => Err(internal(e)),
        };
    }
    match queue.enqueue(request, principal.actor()).await {
        Ok(run_id) => Ok((
            StatusCode::ACCEPTED,
            Json(json!({"run_id": run_id, "state": run_state::QUEUED})),
        )
            .into_response()),
        Err(EnqueueError::Full) => Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": "run queue full; retry later"})),
        )
            .into_response()),
        Err(EnqueueError::Store(e)) => Err(internal(e)),
    }
}

/// `POST /api/runs/{id}/stop` — e-stop a run: a running experiment's token
/// is cancelled (the runner stops before the next activity and runs
/// rollbacks); a still-queued run is cancelled before it starts. 404 when
/// the run is unknown, 409 when it already reached a terminal state.
pub async fn stop(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    let Some(queue) = state.runs_handle() else {
        return Err(unavailable("run queue is not wired"));
    };
    match queue.stop(&id, principal.actor().as_deref()).await {
        Ok(()) => Ok(Json(json!({"run_id": id, "stop": "requested"}))),
        Err(StopError::NotFound) => Err(not_found(format!("unknown run id {id:?}"))),
        Err(StopError::Terminal(state)) => Err((
            StatusCode::CONFLICT,
            Json(json!({"error": "run already terminal", "state": state})),
        )
            .into_response()),
        Err(StopError::Store(e)) => Err(internal(e)),
    }
}

/// Active (non-terminal) run states the global halt e-stops.
const HALTABLE_STATES: &[&str] = &[
    run_state::QUEUED,
    run_state::VALIDATING,
    run_state::RUNNING,
    run_state::STOPPING,
    run_state::PENDING_APPROVAL,
];

/// `POST /api/runs/stop-all` — the global halt: e-stop every active run.
/// Running experiments cancel at the next activity boundary and run their
/// rollbacks; queued runs are cancelled before they start; gated runs parked
/// in `pending_approval` are aborted before dispatch. Each stopped run's
/// audit trail records the halting principal on its `stop_requested` event.
/// Runs in an environment outside the principal's scopes are not touched
/// (same rule as the run list). Idempotent: a run that reached a terminal
/// state between listing and stopping is counted as skipped, not an error.
/// A store error on one run does not abort the halt: the response is always
/// a 200 summary — `{requested, stopped, skipped_terminal, failed}` with
/// `failed` listing `{run_id, error}` for each run that could not be
/// stopped (retry the call to halt them).
pub async fn stop_all(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Value>, Response> {
    let Some(queue) = state.runs_handle() else {
        return Err(unavailable("run queue is not wired"));
    };
    let scopes = principal.env_scopes.clone();
    let ids = with_reader(&state.db_path, move |reader| {
        let state_list = HALTABLE_STATES
            .iter()
            .map(|s| sql_string(s))
            .collect::<Vec<_>>()
            .join(", ");
        if scopes.is_empty() {
            return reader
                .query_json_rows(&format!(
                    "SELECT id FROM runs WHERE state IN ({state_list})"
                ))
                .map_err(|e| e.to_string());
        }
        let env_list = scopes
            .iter()
            .map(|s| sql_string(s))
            .collect::<Vec<_>>()
            .join(", ");
        reader
            .query_json_rows(&format!(
                "SELECT r.id FROM runs r \
                 LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env \
                            FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id \
                 WHERE r.state IN ({state_list}) \
                   AND (e.env IN ({env_list}) OR r.experiment_id IS NULL)"
            ))
            .map_err(|e| e.to_string())
    })
    .await?;
    let actor = principal.actor();
    let summary = halt_runs(&ids, |id| {
        let actor = actor.clone();
        async move { queue.stop(&id, actor.as_deref()).await }
    })
    .await;
    tracing::warn!(
        requested = summary.requested,
        stopped = summary.stopped,
        skipped_terminal = summary.skipped_terminal,
        failed = summary.failed.len(),
        actor = actor.as_deref().unwrap_or("synthetic"),
        "global halt requested"
    );
    Ok(Json(json!({
        "requested": summary.requested,
        "stopped": summary.stopped,
        "skipped_terminal": summary.skipped_terminal,
        "failed": summary.failed,
    })))
}

/// The outcome of one `stop-all` pass: per-run results, never an early
/// abort — one run's store error must not leave the remaining runs active
/// behind a misleading 500.
struct HaltSummary {
    requested: usize,
    stopped: usize,
    skipped_terminal: usize,
    /// `{run_id, error}` for each run whose stop failed at the store.
    failed: Vec<Value>,
}

/// E-stop every listed run, collecting per-run outcomes. Store errors are
/// recorded against the run and the loop continues; runs that turned
/// terminal (or vanished) between listing and stopping count as skipped.
async fn halt_runs<F, Fut>(ids: &[Value], stop: F) -> HaltSummary
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<(), StopError>>,
{
    let mut summary = HaltSummary {
        requested: ids.len(),
        stopped: 0,
        skipped_terminal: 0,
        failed: Vec::new(),
    };
    for row in ids {
        let id = row["id"].as_str().unwrap_or_default().to_string();
        match stop(id.clone()).await {
            Ok(()) => summary.stopped += 1,
            Err(StopError::Terminal(_) | StopError::NotFound) => summary.skipped_terminal += 1,
            Err(StopError::Store(e)) => {
                tracing::warn!(run = %id, error = %e, "global halt: stop failed; continuing with the remaining runs");
                summary.failed.push(json!({"run_id": id, "error": e}));
            }
        }
    }
    summary
}

// ---------------------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    /// One run's store failure must not abort the halt: the remaining runs
    /// are still stopped and the failure is reported per-run.
    #[tokio::test]
    async fn halt_runs_continues_past_a_failing_run() {
        let ids = vec![
            json!({"id": "run-a"}),
            json!({"id": "run-broken"}),
            json!({"id": "run-gone"}),
            json!({"id": "run-terminal"}),
            json!({"id": "run-b"}),
        ];
        let summary = halt_runs(&ids, |id| async move {
            match id.as_str() {
                "run-broken" => Err(StopError::Store("writer down".into())),
                "run-gone" => Err(StopError::NotFound),
                "run-terminal" => Err(StopError::Terminal("passed".into())),
                _ => Ok(()),
            }
        })
        .await;
        assert_eq!(summary.requested, 5);
        assert_eq!(summary.stopped, 2, "run-a and run-b are both stopped");
        assert_eq!(summary.skipped_terminal, 2);
        assert_eq!(summary.failed.len(), 1);
        assert_eq!(summary.failed[0]["run_id"], json!("run-broken"));
        assert_eq!(summary.failed[0]["error"], json!("writer down"));
    }
}
