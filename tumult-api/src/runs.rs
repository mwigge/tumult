//! Run-control endpoints (`/api/runs*`) — validate, dry-run, enqueue,
//! e-stop and inspect daemon-managed experiment runs (schema v5
//! `run_registry` / `runs` / `run_audit`).
//!
//! Definitions register through `POST /api/runs/validate`: the exact
//! parse/resolve/validate pipeline the CLI's `tumult run` applies
//! ([`tumult_ingest::prepare_run`]), then a content-hash-deduped row in
//! `run_registry`. `POST /api/runs` enqueues onto the daemon's bounded
//! [`tumult_ingest::RunQueue`] (429 on overload — never silently queued);
//! `POST /api/runs/{id}/stop` cancels the run's e-stop token. All reads
//! run on a fresh read-only connection, all mutations ride the daemon's
//! single-writer channel — this module never opens a write connection.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_ingest::{Batch, EnqueueError, RunRequest, StopError};
use tumult_lake::{run_state, RegisteredDefinition, Writer};

use crate::auth::Principal;
use crate::sql_util::{internal, now_ns, sql_string, with_reader};
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

fn bad_request(msg: String) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response()
}

fn not_found(msg: String) -> Response {
    (StatusCode::NOT_FOUND, Json(json!({"error": msg}))).into_response()
}

fn unavailable(msg: &str) -> Response {
    (StatusCode::SERVICE_UNAVAILABLE, Json(json!({"error": msg}))).into_response()
}

/// SHA-256 hex of a definition — its dedup key; the registry id derives
/// from it (`reg-<first 12 hex>`), so identical TOON always lands on the
/// same registry row.
fn content_hash(toon: &str) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(toon.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Fetch one registered definition by id, or a 404 response.
async fn registry_or_404(
    state: &ApiState,
    registry_id: &str,
) -> Result<RegisteredDefinition, Response> {
    if registry_id.chars().count() > 100 {
        return Err(bad_request("registry id too long".into()));
    }
    let id = registry_id.to_string();
    let def = with_reader(&state.db_path, move |reader| {
        reader.registry_definition(&id).map_err(|e| e.to_string())
    })
    .await?;
    def.ok_or_else(|| not_found(format!("unknown registry id {registry_id:?}")))
}

// ---------------------------------------------------------------------------
// POST /api/runs/validate

/// JSON body: the experiment TOON plus optional template variables.
#[derive(Debug, Deserialize)]
pub struct ValidateRequest {
    toon: String,
    #[serde(default)]
    vars: HashMap<String, String>,
}

/// `GET /api/registry` — registered definitions (metadata only), newest
/// first: the UI's registry picker.
pub async fn registry_list(State(state): State<ApiState>) -> Result<Json<Value>, Response> {
    let rows = with_reader(&state.db_path, |reader| {
        reader.registry_list(500).map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"count": rows.len(), "definitions": rows})))
}

/// `GET /api/registry/{id}` — one definition including the `.toon` source
/// (the UI parses `${var}` placeholders from it for the parameter form).
pub async fn registry_detail(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    let def = registry_or_404(&state, &id).await?;
    Ok(Json(json!({"definition": def})))
}

/// `POST /api/runs/validate` — run the full parse/resolve/validate pipeline
/// and register the definition (content-hash dedup) so it can be dry-run or
/// enqueued by `registry_id`.
pub async fn validate(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<ValidateRequest>,
) -> Result<Json<Value>, Response> {
    if req.toon.chars().count() > 256_000 {
        return Err(bad_request("definition too large (max 256k chars)".into()));
    }
    let (experiment, _env) = match tumult_ingest::prepare_run(&req.toon, &req.vars) {
        Ok(prepared) => prepared,
        Err(e) => {
            return Ok(Json(json!({"valid": false, "error": e})));
        }
    };

    let hash = content_hash(&req.toon);
    let lookup = hash.clone();
    let existing = with_reader(&state.db_path, move |reader| {
        reader.registry_by_hash(&lookup).map_err(|e| e.to_string())
    })
    .await?;
    if let Some(def) = existing {
        return Ok(Json(json!({
            "valid": true,
            "registry_id": def.id,
            "name": def.name,
            "registered": false,
        })));
    }

    // New definition: register through the single-writer channel.
    let Some(ingest) = state.ingest_handle() else {
        return Err(unavailable(
            "run registration is not wired (no ingest handle)",
        ));
    };
    let def = RegisteredDefinition {
        id: format!("reg-{}", &hash[..12]),
        name: experiment.title.clone(),
        definition_toon: req.toon,
        content_hash: hash,
        registered_at_ns: now_ns(),
        // The authenticated principal when auth is enabled; "api" while open.
        registered_by: principal.actor().or_else(|| Some("api".into())),
    };
    let slot = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    let def2 = def.clone();
    ingest
        .write(Batch::Exec(Box::new(move |writer: &Writer| {
            *slot2.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(writer.register_definition(&def2));
            Ok(())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    let result = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    match result {
        Some(Ok(())) => Ok(Json(json!({
            "valid": true,
            "registry_id": def.id,
            "name": def.name,
            "registered": true,
        }))),
        Some(Err(e)) => Err(internal(e.to_string())),
        None => Err(internal("definition registration did not run".into())),
    }
}

// ---------------------------------------------------------------------------
// POST /api/runs/dry-run

/// JSON body: which registered definition, plus optional template variables.
#[derive(Debug, Deserialize)]
pub struct DryRunRequest {
    registry_id: String,
    #[serde(default)]
    vars: HashMap<String, String>,
}

/// `POST /api/runs/dry-run` — the resolved execution plan for a registered
/// definition (title, estimate, baseline, hypothesis probes, method steps in
/// order, guards, rollbacks) with nothing executed — the JSON counterpart of
/// the CLI's `--dry-run` output. The additive `scope` block summarizes the
/// blast radius for the launch preview: declared note, targeted fault
/// actions, guards and the concurrent-fault cap.
pub async fn dry_run(
    State(state): State<ApiState>,
    Json(req): Json<DryRunRequest>,
) -> Result<Json<Value>, Response> {
    let def = registry_or_404(&state, &req.registry_id).await?;
    match tumult_ingest::prepare_run(&def.definition_toon, &req.vars) {
        Err(e) => Ok(Json(json!({"valid": false, "error": e}))),
        Ok((experiment, _env)) => Ok(Json(json!({
            "valid": true,
            "registry_id": def.id,
            "plan": {
                "title": experiment.title,
                "description": experiment.description,
                "tags": experiment.tags,
                "estimate": experiment.estimate,
                "baseline": experiment.baseline,
                "hypothesis": experiment.steady_state_hypothesis,
                "guards": experiment.guards,
                "method": experiment.method,
                "rollbacks": experiment.rollbacks,
                "controls": experiment.controls,
                "regulatory": experiment.regulatory,
                "blast_radius": experiment.blast_radius,
                "scope": scope_of(&experiment),
            },
        }))),
    }
}

/// Provider argument keys that identify what a fault aims at; everything
/// else (durations, rates, flags) stays out of the scope summary.
const TARGET_ARG_KEYS: &[&str] = &[
    "container",
    "host",
    "selector",
    "process",
    "interface",
    "pod",
    "namespace",
];

/// The blast-radius summary of a resolved experiment: the declared note, the
/// fault-injecting method steps with their identifying arguments, the guards
/// and the concurrent-fault cap. Nulls and empty lists stand for "nothing
/// declared" — the block is always present.
fn scope_of(experiment: &tumult_core::types::Experiment) -> Value {
    let actions: Vec<Value> = experiment
        .method
        .iter()
        .filter(|a| a.activity_type == tumult_core::types::ActivityType::Action)
        .map(|a| {
            let (provider, action, targets) = provider_summary(&a.provider);
            json!({
                "step": a.name,
                "provider": provider,
                "action": action,
                "targets": targets,
            })
        })
        .collect();
    let guards: Vec<Value> = experiment
        .guards
        .iter()
        .map(|g| {
            json!({
                "name": g.name,
                "probe": g.probe.name,
                "min_breaches": g.min_breaches,
            })
        })
        .collect();
    json!({
        "blast_radius": experiment.blast_radius,
        "actions": actions,
        "guards": guards,
        "max_concurrent_faults": experiment.max_concurrent_faults,
    })
}

/// One-line provider identity plus the arguments that name its target.
fn provider_summary(provider: &tumult_core::types::Provider) -> (String, String, Value) {
    use tumult_core::types::Provider;
    match provider {
        Provider::Native {
            plugin,
            function,
            arguments,
        }
        | Provider::Script {
            plugin,
            function,
            arguments,
            ..
        } => {
            let targets: serde_json::Map<String, Value> = arguments
                .iter()
                .filter(|(k, _)| TARGET_ARG_KEYS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            (plugin.clone(), function.clone(), Value::Object(targets))
        }
        Provider::Process { path, .. } => (String::from("process"), path.clone(), json!({})),
    }
}

// ---------------------------------------------------------------------------
// POST /api/runs (+ /{id}/stop)

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
/// authenticated principal as actor.
pub async fn create(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<CreateRunRequest>,
) -> Result<Response, Response> {
    let Some(queue) = state.runs_handle() else {
        return Err(unavailable("run queue is not wired"));
    };
    let def = registry_or_404(&state, &req.registry_id).await?;
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

// ---------------------------------------------------------------------------
// GET /api/runs (+ /{id})

#[derive(Debug, Deserialize)]
pub struct ListParams {
    state: Option<String>,
    limit: Option<u32>,
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
    let scopes = principal.env_scopes.clone();
    let rows = with_reader(&state.db_path, move |reader| {
        if scopes.is_empty() {
            return reader
                .runs(state_filter.as_deref(), limit)
                .map_err(|e| e.to_string());
        }
        // The `experiments` analytics table has no env column; spans do.
        let env_list = scopes
            .iter()
            .map(|s| sql_string(s))
            .collect::<Vec<_>>()
            .join(", ");
        let state_clause = state_filter.as_deref().map_or(String::new(), |s| {
            format!("AND r.state = {}", sql_string(s))
        });
        reader
            .query_json_rows(&format!(
                "SELECT r.*, g.name AS definition_name FROM runs r \
                 LEFT JOIN run_registry g ON g.id = r.registry_id \
                 LEFT JOIN (SELECT experiment_id, any_value(target_environment) AS env \
                            FROM spans GROUP BY 1) e ON e.experiment_id = r.experiment_id \
                 WHERE (e.env IN ({env_list}) OR r.experiment_id IS NULL) {state_clause} \
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
        return Err(bad_request("run id too long".into()));
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
        return Err(bad_request("run id too long".into()));
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
