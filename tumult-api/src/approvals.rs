//! Approval workflow endpoints (`/api/approvals`, `/api/runs/{id}/approve`,
//! `/api/runs/{id}/reject`, `/api/runs/{id}/break-glass`) — the HTTP layer
//! of the T10 gated-run flow (ADR-012).
//!
//! Gated runs (tiers T1–T3, classified at request time by `POST /api/runs`)
//! wait in `pending_approval` with a canonical pin, a quorum and a TTL.
//! Approvers record decisions here; segregation of duties (approver ≠
//! requester) and one-decision-per-approver are enforced by the writer and
//! mapped to 403/409. T3 approvals re-run the tumult-autopilot gate against
//! current ambient facts — fail closed: no policy, a non-Enact verdict, or
//! an error refuses the approval (422), and a Veto can never be overridden
//! by an approval (break-glass only). Dispatch funnels through
//! [`RunQueue::dispatch_approved`], which re-reads every check from the
//! store; break-glass (Admin, mandatory justification) bypasses quorum and
//! TTL there and opens a retrospective manual-evidence draft as compliance
//! debt. All reads run on a fresh read-only connection, all mutations ride
//! the daemon's single-writer channel — this module never opens a write
//! connection.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_ingest::approvals::{evaluate_t3_gate, introspect, GateOutcome};
use tumult_ingest::runs::DispatchError;
use tumult_ingest::{Batch, RunQueue};
use tumult_lake::approvals::decision;
use tumult_lake::{
    rollback_status, run_state, ApprovalDecision, ExerciseType, ManualOutcome, NewManualExperiment,
    Writer,
};

use crate::auth::Principal;
use crate::{internal, now_ns, with_reader, ApiState};

fn bad_request(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response()
}

fn not_found(msg: String) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"error": msg}))).into_response()
}

fn unavailable(msg: &str) -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": msg}))).into_response()
}

fn conflict(body: Value) -> Response {
    (StatusCode::CONFLICT, Json(body)).into_response()
}

fn unprocessable(body: Value) -> Response {
    (StatusCode::UNPROCESSABLE_ENTITY, Json(body)).into_response()
}

/// Run `f` on the daemon's single writer; the closure's own outcome travels
/// back in a slot (the `Batch::Exec` ack itself always succeeds), so typed
/// write failures keep their message for HTTP mapping.
async fn exec_write<T>(
    state: &ApiState,
    f: impl FnOnce(&Writer) -> Result<T, String> + Send + 'static,
) -> Result<Result<T, String>, Response>
where
    T: Send + 'static,
{
    let Some(ingest) = state.ingest_handle() else {
        return Err(unavailable(
            "approval writes are not wired (no ingest handle)",
        ));
    };
    let slot = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    ingest
        .write(Batch::Exec(Box::new(move |writer: &Writer| {
            *slot2.lock().unwrap_or_else(|e| e.into_inner()) = Some(f(writer));
            Ok(())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    let result = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    match result {
        Some(r) => Ok(r),
        None => Err(internal("approval write did not run".into())),
    }
}

/// Map a decision-write failure: self-approval → 403 (segregation of
/// duties), a second decision by the same approver → 409, anything else 500.
fn decision_error(msg: String) -> Response {
    if msg.contains("self-approval") {
        (StatusCode::FORBIDDEN, Json(json!({"error": msg}))).into_response()
    } else if msg.contains("already decided") {
        conflict(json!({"error": msg}))
    } else {
        internal(msg)
    }
}

/// The run row for `id`, requiring it to exist and await approval.
/// Anything else — unknown, queued, running, terminal (including `expired`)
/// — is a 404/409; an expired request must be re-requested.
async fn pending_run(state: &ApiState, id: &str) -> Result<Value, Response> {
    let lookup = id.to_string();
    let run = with_reader(&state.db_path, move |reader| {
        reader.run_get(&lookup).map_err(|e| e.to_string())
    })
    .await?;
    let Some(run) = run else {
        return Err(not_found(format!("unknown run id {id:?}")));
    };
    let run_state_str = run["state"].as_str().unwrap_or_default().to_string();
    if run_state_str != run_state::PENDING_APPROVAL {
        return Err(conflict(
            json!({"error": "run not awaiting approval", "state": run_state_str}),
        ));
    }
    Ok(run)
}

/// Whether the current UTC hour is in 7..19 — the T3 gate's
/// `within_business_hours` ambient fact.
fn within_business_hours() -> bool {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let hour = (secs % 86_400) / 3_600;
    (7..19).contains(&hour)
}

/// The T3 gate's pre-decision check, evaluated read-side.
enum GateCheck {
    /// Not T3, or T3 with an Enact verdict (the policy hash rides into the
    /// audit detail).
    Clear(Option<String>),
    /// The approval TTL lapsed; the run is flipped terminal before the 409.
    Expired,
    /// The gate vetoed — no approval can override this (break-glass only).
    Veto(String),
    /// The gate returned a non-Enact verdict (fail closed).
    NotEnact(String),
    /// No policy loaded (or another gate failure) — fail closed.
    Unavailable(String),
}

/// Fetch the approval request and, for T3, re-run the autopilot gate
/// against current ambient facts. A missing approval request is a 500 via
/// [`with_reader`]'s error mapping.
async fn gate_check(state: &ApiState, id: &str, run: &Value) -> Result<GateCheck, Response> {
    let lookup = id.to_string();
    let run = run.clone();
    let policy = state.autopilot_policy();
    with_reader(&state.db_path, move |reader| {
        let approval = reader
            .approval_request(&lookup)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("no approval request for run {lookup}"))?;
        if now_ns() > approval["expires_at_ns"].as_i64().unwrap_or(0) {
            return Ok(GateCheck::Expired);
        }
        if approval["tier"].as_str() != Some("T3") {
            return Ok(GateCheck::Clear(None));
        }
        // Rebuild the introspection from the pinned inputs (registry
        // definition + stored params), exactly as dispatch re-resolves them.
        let definition = reader
            .registry_definition(run["registry_id"].as_str().unwrap_or_default())
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "registry row missing".to_string())?;
        let vars: HashMap<String, String> = run["params_json"]
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();
        let (experiment, _env) = tumult_ingest::prepare_run(&definition.definition_toon, &vars)?;
        let intro = introspect(&experiment);
        let now = now_ns();
        let runs_today = reader
            .query_json_rows(&format!(
                "SELECT COUNT(*) AS n FROM runs WHERE started_at_ns > {}",
                now - 24 * 3_600 * 1_000_000_000
            ))
            .map_err(|e| e.to_string())?
            .first()
            .and_then(|r| r["n"].as_u64())
            .unwrap_or(0) as u32;
        let concurrent_experiments = reader.active_runs().map_err(|e| e.to_string())?.len() as u32;
        let ambient = tumult_autopilot::AmbientContext {
            open_deviation_for_target: false,
            runs_today,
            hours_since_last_run_on_service: None,
            within_business_hours: within_business_hours(),
            concurrent_experiments,
            guard_telemetry_ok: None,
        };
        let env = approval["env"].as_str().unwrap_or("dev").to_string();
        let target = approval["target"].as_str().map(str::to_string);
        Ok(
            match evaluate_t3_gate(
                policy.as_deref(),
                &lookup,
                &intro,
                &env,
                target.as_deref(),
                &ambient,
            ) {
                GateOutcome::Enact { policy_hash, .. } => GateCheck::Clear(Some(policy_hash)),
                GateOutcome::Veto { rule } => GateCheck::Veto(rule),
                GateOutcome::NotEnact { verdict } => GateCheck::NotEnact(verdict),
                GateOutcome::Unavailable { reason } => GateCheck::Unavailable(reason),
            },
        )
    })
    .await
}

/// Dispatch an approved (or break-glass-overridden) run and shape the
/// response: quorum met → 200 `queued`; quorum short → 200
/// `pending_approval` (normal for the first of two T3 approvals); overload
/// → 429; anything else 500.
async fn dispatch_response(
    queue: &RunQueue,
    id: &str,
    break_glass: bool,
) -> Result<Response, Response> {
    match queue.dispatch_approved(id).await {
        Ok(()) => {
            let mut body = json!({"run_id": id, "state": run_state::QUEUED});
            if break_glass {
                body["break_glass"] = json!(true);
            }
            Ok((StatusCode::OK, Json(body)).into_response())
        }
        Err(DispatchError::Approval(_)) => Ok((
            StatusCode::OK,
            Json(json!({"run_id": id, "state": run_state::PENDING_APPROVAL})),
        )
            .into_response()),
        Err(DispatchError::Full) => Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({"error": "run queue full; retry before the approval TTL lapses"})),
        )
            .into_response()),
        Err(DispatchError::NotPending) => Err(internal("run is no longer pending approval".into())),
        Err(DispatchError::Store(e)) => Err(internal(e)),
    }
}

// ---------------------------------------------------------------------------
// GET /api/approvals

/// `GET /api/approvals` — the pending approval queue (oldest first), each
/// entry carrying its request, definition name and approvals collected.
pub async fn queue(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let rows = with_reader(&state.db_path, |reader| {
        reader.approvals_queue().map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "queue": rows})))
}

// ---------------------------------------------------------------------------
// POST /api/runs/{id}/approve

/// JSON body for approve/reject: an optional reviewer note.
#[derive(Debug, Deserialize)]
pub struct DecisionRequest {
    note: Option<String>,
}

/// `POST /api/runs/{id}/approve` — record an approval and dispatch when the
/// quorum is met. 404 unknown run, 409 not awaiting approval (or TTL
/// lapsed — the run is then flipped to terminal `expired`), 403
/// self-approval, 409 a second decision by the same approver. T3 re-runs
/// the autopilot gate first: a veto can never be approved past (422,
/// break-glass only), and no policy / a non-Enact verdict fails closed
/// (422). Quorum short after the decision is a normal 200
/// `pending_approval` (the first of two T3 approvals).
pub async fn approve(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<DecisionRequest>,
) -> Result<Response, Response> {
    let Some(queue) = state.runs_handle() else {
        return Err(unavailable("run queue is not wired"));
    };
    let run = pending_run(&state, &id).await?;
    let policy_hash = match gate_check(&state, &id, &run).await? {
        GateCheck::Clear(hash) => hash,
        GateCheck::Expired => {
            let id2 = id.clone();
            let result = exec_write(&state, move |writer| {
                writer
                    .finish_run(
                        &id2,
                        run_state::EXPIRED,
                        None,
                        Some(rollback_status::NOT_NEEDED),
                        Some("approval TTL lapsed"),
                    )
                    .map_err(|e| e.to_string())
            })
            .await?;
            if let Err(e) = result {
                return Err(internal(e));
            }
            return Err(conflict(
                json!({"error": "approval expired", "state": run_state::EXPIRED}),
            ));
        }
        GateCheck::Veto(rule) => {
            let id2 = id.clone();
            let actor = principal.actor();
            let rule2 = rule.clone();
            let result = exec_write(&state, move |writer| {
                writer
                    .insert_run_audit(&id2, "gate_veto", Some(&rule2), actor.as_deref())
                    .map_err(|e| e.to_string())
            })
            .await?;
            if let Err(e) = result {
                return Err(internal(e));
            }
            return Err(unprocessable(json!({
                "error": "autopilot gate vetoed this run — an approval cannot override a veto (break-glass only)",
                "rule": rule,
            })));
        }
        GateCheck::NotEnact(verdict) => {
            return Err(unprocessable(
                json!({"error": "autopilot gate did not return Enact", "verdict": verdict}),
            ));
        }
        GateCheck::Unavailable(reason) => {
            return Err(unprocessable(
                json!({"error": format!("autopilot gate unavailable (fail closed): {reason}")}),
            ));
        }
    };

    let decision_row = ApprovalDecision {
        run_id: id.clone(),
        approver: principal.username.clone(),
        decision: decision::APPROVED.into(),
        note: req.note.clone(),
        decided_at_ns: now_ns(),
    };
    // The T3 audit detail names the exact policy that enacted the approval.
    let detail = match (&policy_hash, &req.note) {
        (Some(hash), Some(note)) => Some(format!("policy_hash {hash}; note: {note}")),
        (Some(hash), None) => Some(format!("policy_hash {hash}")),
        (None, _) => req.note.clone(),
    };
    let id2 = id.clone();
    let actor = principal.actor();
    let result = exec_write(&state, move |writer| {
        writer
            .insert_approval_decision(&decision_row)
            .map_err(|e| e.to_string())?;
        writer
            .insert_run_audit(&id2, "approved", detail.as_deref(), actor.as_deref())
            .map_err(|e| e.to_string())
    })
    .await?;
    if let Err(e) = result {
        return Err(decision_error(e));
    }
    dispatch_response(queue, &id, false).await
}

// ---------------------------------------------------------------------------
// POST /api/runs/{id}/reject

/// `POST /api/runs/{id}/reject` — record a rejection and flip the run
/// terminal `rejected` (no autopilot gate — refusal needs no policy). Same
/// 404/409/403 state and segregation-of-duties checks as approve. The
/// `finish_run` write records its own actor-less `rejected` audit event;
/// the explicit `insert_run_audit` after it records the rejecting actor —
/// both are kept, the actor-carrying one is authoritative.
pub async fn reject(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<DecisionRequest>,
) -> Result<Response, Response> {
    pending_run(&state, &id).await?;
    let decision_row = ApprovalDecision {
        run_id: id.clone(),
        approver: principal.username.clone(),
        decision: decision::REJECTED.into(),
        note: req.note.clone(),
        decided_at_ns: now_ns(),
    };
    let id2 = id.clone();
    let actor = principal.actor();
    let note = req.note.clone();
    let result = exec_write(&state, move |writer| {
        writer
            .insert_approval_decision(&decision_row)
            .map_err(|e| e.to_string())?;
        writer
            .finish_run(
                &id2,
                run_state::REJECTED,
                None,
                Some(rollback_status::NOT_NEEDED),
                note.as_deref(),
            )
            .map_err(|e| e.to_string())?;
        writer
            .insert_run_audit(&id2, "rejected", note.as_deref(), actor.as_deref())
            .map_err(|e| e.to_string())
    })
    .await?;
    if let Err(e) = result {
        return Err(decision_error(e));
    }
    Ok((
        StatusCode::OK,
        Json(json!({"run_id": id, "state": run_state::REJECTED})),
    )
        .into_response())
}

// ---------------------------------------------------------------------------
// POST /api/runs/{id}/break-glass

/// JSON body for break-glass: the mandatory justification (min 10 chars).
#[derive(Debug, Deserialize)]
pub struct BreakGlassRequest {
    justification: String,
}

/// `POST /api/runs/{id}/break-glass` — admin override of the approval
/// quorum and TTL (ADR-012): stamps the request `break_glass`, audits the
/// override, opens a retrospective manual-evidence draft (unscored,
/// status stays `draft`) as compliance debt, then dispatches — the pin
/// re-verification in the worker still applies. 400 on a justification
/// under 10 characters, 404 unknown run, 409 when the run is not awaiting
/// approval (an expired request must be re-requested).
pub async fn break_glass(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<BreakGlassRequest>,
) -> Result<Response, Response> {
    let Some(queue) = state.runs_handle() else {
        return Err(unavailable("run queue is not wired"));
    };
    if req.justification.trim().chars().count() < 10 {
        return Err(bad_request("justification too short (min 10 chars)".into()));
    }
    pending_run(&state, &id).await?;

    // The override and its audit event.
    let id2 = id.clone();
    let actor = principal.actor();
    let username = principal.username.clone();
    let justification = req.justification.clone();
    let result = exec_write(&state, move |writer| {
        writer
            .mark_break_glass(&id2, &username, &justification)
            .map_err(|e| e.to_string())?;
        writer
            .insert_run_audit(&id2, "overridden", Some(&justification), actor.as_deref())
            .map_err(|e| e.to_string())
    })
    .await?;
    if let Err(e) = result {
        return Err(internal(e));
    }

    // The pinned request's env/target/pin flavor the retrospective record.
    let lookup = id.clone();
    let approval = with_reader(&state.db_path, move |reader| {
        reader.approval_request(&lookup).map_err(|e| e.to_string())
    })
    .await?;

    // Compliance debt: the mandatory retrospective review item, as an
    // unscored manual-evidence draft.
    let pin = approval
        .as_ref()
        .and_then(|a| a["pin_hash"].as_str())
        .unwrap_or("unknown")
        .to_string();
    let new = NewManualExperiment {
        experiment_name: format!("Break-glass retrospective — run {id}"),
        exercise_type: ExerciseType::Other,
        executed_at_ns: now_ns(),
        hypothesis: format!("Break-glass override was required: {}", req.justification),
        method: format!(
            "Approval quorum and TTL bypassed by break-glass dispatch of run {id} (pin {pin})"
        ),
        outcome: ManualOutcome::Inconclusive,
        hypothesis_met: None,
        findings: None,
        action_items: vec![format!(
            "Retrospective review of break-glass dispatch for run {id}"
        )],
        target_system: approval
            .as_ref()
            .and_then(|a| a["target"].as_str())
            .map(str::to_string),
        target_environment: approval
            .as_ref()
            .and_then(|a| a["env"].as_str())
            .map(str::to_string),
        blast_radius: None,
        recovery_time_s: None,
        duration_s: None,
        entered_by: principal.username.clone(),
        attestation: format!(
            "I attest this break-glass override was justified: {}",
            req.justification
        ),
        renewal_due_ns: None,
        framework_refs: vec![],
    };
    let result = exec_write(&state, move |writer| {
        writer.create_manual_draft(&new).map_err(|e| e.to_string())
    })
    .await?;
    if let Err(e) = result {
        return Err(internal(e));
    }

    dispatch_response(queue, &id, true).await
}
