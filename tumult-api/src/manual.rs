//! Manual evidence endpoints (`/api/manual/*`).
//!
//! Mutations ride the daemon's single-writer channel via
//! [`tumult_ingest::Batch::Exec`] — the API never opens a write connection
//! of its own. The acting identity (`entered_by` on create/update, `by` on
//! submit, `reviewer` on verify/reject, `added_by` on attach) comes from the
//! authenticated [`Principal`]: when auth is enabled the request-body fields
//! are ignored; while auth is open (the synthetic principal, see
//! [`crate::auth`]) the body fields are required, exactly as before auth
//! existed. The store enforces the lifecycle rules (draft mutability,
//! attestation on submit, reviewer ≠ enterer) regardless.
//!
//! Every request body refuses a field it does not know (422, naming the
//! field), so a client never takes a dropped value for a stored one.
//!
//! Verify and reject are sign-offs. A record stays submitted until it has
//! as many approvals, each from another user, as the house parameter
//! `manual_required_signoffs` asks for its exercise type (one by
//! default). A signer who entered or changed the record, or signed it off
//! before, is refused with 422. `GET .../audit/verify` checks the seal,
//! the audit trail, the sign-offs and the attachments again.
//!
//! Evidence is a link sent as JSON, or the bytes of a file sent as
//! `application/octet-stream` ([`crate::manual_files`]).

use crate::error::ApiError;
use std::sync::{Arc, Mutex};

use axum::extract::{FromRequest, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_ingest::Batch;
use tumult_lake::{
    ActionItem, AttachmentKind, ExerciseDetail, ExerciseLink, ExerciseStep, ExerciseType, LinkKind,
    ManualError, ManualOutcome, NewManualExperiment, Participant, PlanError, Reader, SignOff,
    SignOffDecision, SignOffOutcome, StepResult, Store, Writer,
};

use crate::auth::Principal;
use crate::sql_util::{internal, with_reader};
use crate::ApiState;

/// The acting identity for a mutation: the authenticated principal's
/// username when auth is enabled, else the request-supplied field (required,
/// as before auth existed).
pub(crate) fn actor_or(
    principal: &Principal,
    field: Option<String>,
    name: &str,
) -> Result<String, ManualError> {
    if !principal.synthetic {
        return Ok(principal.username.clone());
    }
    field
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| ManualError::Invalid(format!("'{name}' must not be empty")))
}

/// Map a lifecycle error to an HTTP response. A record that names a plan
/// it cannot fulfil is 422, and so is a sign-off the rules refuse.
pub(crate) fn manual_error(err: &ManualError) -> ApiError {
    let status = match err {
        ManualError::Invalid(_) | ManualError::Plan(PlanError::Invalid(_)) => {
            StatusCode::BAD_REQUEST
        }
        ManualError::NotFound(_) | ManualError::Plan(PlanError::NotFound(_)) => {
            StatusCode::NOT_FOUND
        }
        ManualError::WrongStatus { .. } | ManualError::Plan(PlanError::WrongStatus { .. }) => {
            StatusCode::CONFLICT
        }
        ManualError::Plan(PlanError::Unusable(_))
        | ManualError::SelfReview
        | ManualError::AlreadySigned(_) => StatusCode::UNPROCESSABLE_ENTITY,
        ManualError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({"error": err.to_string()})))
        .into_response()
        .into()
}

/// What a write touches, read before it runs to check the caller's
/// environment scopes.
pub(crate) enum Subject {
    /// A record: 404 outside the scopes. The plan it names is checked too.
    Record(String),
    /// A plan: 404 outside the scopes.
    Plan(String),
    /// The plan a record names: 422 outside the scopes, as a plan that
    /// does not exist.
    NamedPlan(String),
}

pub(crate) fn in_scopes(env: Option<&str>, scopes: &[String]) -> bool {
    env.is_some_and(|env| scopes.iter().any(|s| s == env))
}

/// Refuse `subject` unless its environment is one of `scopes`.
fn check_scope(reader: &Reader, scopes: &[String], subject: &Subject) -> Result<(), ManualError> {
    let plan_env = |id: &str| -> Result<Option<String>, ManualError> {
        let detail = reader.manual_plan_detail(id)?;
        Ok(detail.and_then(|d| d.plan["target_environment"].as_str().map(str::to_owned)))
    };
    match subject {
        Subject::Record(id) => {
            let detail = reader.manual_experiment_detail(id)?;
            let row = detail.as_ref().map(|d| &d.experiment);
            if !in_scopes(row.and_then(|r| r["target_environment"].as_str()), scopes) {
                return Err(ManualError::NotFound(id.clone()));
            }
            match row.and_then(|r| r["plan_id"].as_str()) {
                Some(plan) => check_scope(reader, scopes, &Subject::NamedPlan(plan.to_owned())),
                None => Ok(()),
            }
        }
        Subject::Plan(id) => {
            if in_scopes(plan_env(id)?.as_deref(), scopes) {
                return Ok(());
            }
            Err(PlanError::NotFound(id.clone()).into())
        }
        Subject::NamedPlan(id) => {
            if in_scopes(plan_env(id)?.as_deref(), scopes) {
                return Ok(());
            }
            Err(PlanError::Unusable(format!("plan '{id}' does not exist")).into())
        }
    }
}

/// Run a typed manual-evidence mutation on the single writer and surface
/// the typed result. `resource_id` is the record it touches, if any.
pub(crate) async fn exec_manual<T>(
    state: &ApiState,
    principal: &Principal,
    resource_id: Option<&str>,
    f: impl FnOnce(&Writer) -> Result<T, ManualError> + Send + 'static,
) -> Result<T, ApiError>
where
    T: Send + 'static,
{
    let subjects = resource_id
        .map(|id| Subject::Record(id.to_owned()))
        .into_iter()
        .collect();
    exec_scoped(state, principal, subjects, f).await
}

/// The plans the record bodies name, to check against the caller's scopes.
fn named_plans<'a>(plan_ids: impl IntoIterator<Item = &'a Option<String>>) -> Vec<Subject> {
    plan_ids
        .into_iter()
        .flatten()
        .map(|id| Subject::NamedPlan(id.clone()))
        .collect()
}

/// Run a typed manual-evidence mutation on the single writer and surface
/// the typed result (the `Batch::Exec` closure itself always "succeeds" so
/// the channel ack stays clean; the real outcome travels in `slot`). Each
/// of `subjects` must lie in the caller's environment scopes. Every
/// mutation that succeeds clears the Star Map cache.
pub(crate) async fn exec_scoped<T>(
    state: &ApiState,
    principal: &Principal,
    subjects: Vec<Subject>,
    f: impl FnOnce(&Writer) -> Result<T, ManualError> + Send + 'static,
) -> Result<T, ApiError>
where
    T: Send + 'static,
{
    let Some(ingest) = state.ingest_handle() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "manual evidence writes are not wired (no ingest handle)"})),
        )
            .into_response()
            .into());
    };
    let scopes = principal.env_scopes.clone();
    let db = state.db_path.as_ref().clone();
    let slot = Arc::new(Mutex::new(None));
    let slot2 = Arc::clone(&slot);
    ingest
        .write(Batch::Exec(Box::new(move |writer: &Writer| {
            // Resolve scope inside the serialized writer operation: an edit cannot
            // move the record between authorization and mutation.
            let result = (|| {
                if !scopes.is_empty() && !subjects.is_empty() {
                    let reader = Store::at(&db).read_only()?;
                    for subject in &subjects {
                        check_scope(&reader, &scopes, subject)?;
                    }
                }
                f(writer)
            })();
            *slot2.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            Ok(())
        })))
        .await
        .map_err(|e| internal(e.to_string()))?;
    let result = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
    match result {
        Some(Ok(v)) => {
            // Manual records are evidence on the Star Map: no cached map
            // may outlive a change to one.
            state.starmap_cache.clear();
            Ok(v)
        }
        Some(Err(e)) => Err(manual_error(&e)),
        None => Err(internal("manual operation did not run".into())),
    }
}

/// JSON body for create/update: content fields of a manual test record.
/// Enum-typed fields arrive as their lowercase string forms. `entered_by` is
/// ignored when auth is enabled (the principal's username wins); while auth
/// is open it is required. `recovery_time_s` is the actual RTO.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManualRecordRequest {
    experiment_name: String,
    exercise_type: String,
    executed_at_ns: i64,
    hypothesis: String,
    method: String,
    outcome_status: String,
    hypothesis_met: Option<bool>,
    findings: Option<String>,
    #[serde(default)]
    action_items: Vec<String>,
    target_system: Option<String>,
    target_environment: Option<String>,
    blast_radius: Option<String>,
    recovery_time_s: Option<f64>,
    duration_s: Option<f64>,
    entered_by: Option<String>,
    attestation: String,
    renewal_due_ns: Option<i64>,
    #[serde(default)]
    framework_refs: Vec<String>,
    /// The planned RTO, in seconds.
    rto_target_s: Option<f64>,
    /// The planned RPO: the seconds of committed writes from before the
    /// outage that may be lost, as the automated RPO metric means it.
    rpo_target_s: Option<f64>,
    /// The actual RPO: the seconds of committed writes from before the
    /// outage that were lost.
    rpo_measured_s: Option<f64>,
    /// Whether the data was checked after the recovery: `true` complete,
    /// `false` loss or corruption found, absent not checked.
    data_verified: Option<bool>,
    #[serde(default)]
    participants: Vec<ParticipantRequest>,
    /// The timeline, in the order the steps were taken.
    #[serde(default)]
    steps: Vec<StepRequest>,
    /// The actions with owners. A new client sends these: they are the
    /// actions of a record from schema v22 on. `action_items` is the older
    /// list of plain texts, still stored for the clients that send it.
    #[serde(default)]
    actions: Vec<ActionRequest>,
    /// What the exercise tested.
    #[serde(default)]
    links: Vec<LinkRequest>,
    /// The plan the exercise fulfils once the record is verified: a
    /// planned plan of the same exercise type.
    plan_id: Option<String>,
}

/// One person who took part, and their role.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParticipantRequest {
    name: String,
    role: Option<String>,
}

/// One step of the timeline.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepRequest {
    title: String,
    actor: Option<String>,
    started_at_ns: Option<i64>,
    ended_at_ns: Option<i64>,
    /// `done`, `failed` or `skipped`.
    result: Option<String>,
    note: Option<String>,
}

/// One action with the person who owns it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionRequest {
    title: String,
    owner: String,
    due_at_ns: Option<i64>,
    /// A ticket key or URL that tracks the action.
    ticket: Option<String>,
}

/// One link to what the exercise tested.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkRequest {
    /// `function`, `entity`, `registry`, `run` or `gameday`.
    kind: String,
    linked_id: String,
    label: Option<String>,
}

impl StepRequest {
    fn into_step(self) -> Result<ExerciseStep, ManualError> {
        Ok(ExerciseStep {
            title: self.title,
            actor: self.actor,
            started_at_ns: self.started_at_ns,
            ended_at_ns: self.ended_at_ns,
            result: self.result.as_deref().map(StepResult::parse).transpose()?,
            note: self.note,
        })
    }
}

impl LinkRequest {
    fn into_link(self) -> Result<ExerciseLink, ManualError> {
        Ok(ExerciseLink {
            kind: LinkKind::parse(&self.kind)?,
            linked_id: self.linked_id,
            label: self.label,
        })
    }
}

impl From<ParticipantRequest> for Participant {
    fn from(p: ParticipantRequest) -> Self {
        Self {
            name: p.name,
            role: p.role,
        }
    }
}

impl From<ActionRequest> for ActionItem {
    fn from(a: ActionRequest) -> Self {
        Self {
            title: a.title,
            owner: a.owner,
            due_at_ns: a.due_at_ns,
            ticket: a.ticket,
        }
    }
}

impl ManualRecordRequest {
    fn into_new(self, entered_by: String) -> Result<NewManualExperiment, ManualError> {
        let exercise = ExerciseDetail {
            rto_target_s: self.rto_target_s,
            rpo_target_s: self.rpo_target_s,
            rpo_measured_s: self.rpo_measured_s,
            data_verified: self.data_verified,
            participants: self
                .participants
                .into_iter()
                .map(Participant::from)
                .collect(),
            steps: self
                .steps
                .into_iter()
                .map(StepRequest::into_step)
                .collect::<Result<_, _>>()?,
            actions: self.actions.into_iter().map(ActionItem::from).collect(),
            links: self
                .links
                .into_iter()
                .map(LinkRequest::into_link)
                .collect::<Result<_, _>>()?,
            plan_id: self.plan_id,
        };
        Ok(NewManualExperiment {
            experiment_name: self.experiment_name,
            exercise_type: ExerciseType::parse(&self.exercise_type)?,
            executed_at_ns: self.executed_at_ns,
            hypothesis: self.hypothesis,
            method: self.method,
            outcome: ManualOutcome::parse(&self.outcome_status)?,
            hypothesis_met: self.hypothesis_met,
            findings: self.findings,
            action_items: self.action_items,
            target_system: self.target_system,
            target_environment: self.target_environment,
            blast_radius: self.blast_radius,
            recovery_time_s: self.recovery_time_s,
            duration_s: self.duration_s,
            entered_by,
            attestation: self.attestation,
            renewal_due_ns: self.renewal_due_ns,
            framework_refs: self.framework_refs,
            exercise,
        })
    }
}

fn bad_request(err: ManualError) -> ApiError {
    manual_error(&err)
}

/// `POST /api/manual/experiments` — create a draft record.
pub async fn create(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<ManualRecordRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let entered_by =
        actor_or(&principal, req.entered_by.clone(), "entered_by").map_err(bad_request)?;
    check_environment(&principal, req.target_environment.as_deref())?;
    let subjects = named_plans([&req.plan_id]);
    let new = req.into_new(entered_by).map_err(bad_request)?;
    let id = exec_scoped(&state, &principal, subjects, move |w| {
        w.create_manual_draft(&new)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(json!({"id": id}))))
}

#[derive(Deserialize)]
pub struct ListParams {
    status: Option<String>,
}

/// `GET /api/manual/experiments?status=` — list records (newest first).
pub async fn list(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, ApiError> {
    let status = params.status.clone();
    let rows = with_reader(&state.db_path, move |reader| {
        let mut rows = reader
            .manual_experiments(status.as_deref())
            .map_err(|e| e.to_string())?;
        rows.retain(|row| {
            principal.env_allowed(row["target_environment"].as_str().unwrap_or_default())
        });
        Ok(rows)
    })
    .await?;
    Ok(Json(json!({"records": rows})))
}

/// The approving sign-offs a record has, and how many its exercise type
/// needs under the house parameters.
fn signoff_counts(state: &ApiState, record: &Value, signoffs: &[Value]) -> (u32, u32) {
    let exercise_type = record["exercise_type"].as_str().unwrap_or_default();
    let required = state
        .score_runtime()
        .params()
        .required_signoffs(exercise_type);
    let approvals = signoffs
        .iter()
        .filter(|s| s["decision"] == SignOffDecision::Approved.as_str())
        .count();
    (u32::try_from(approvals).unwrap_or(u32::MAX), required)
}

/// The sign-offs still needed to verify a record: none once it is
/// verified or rejected.
fn still_needed(status: &str, approvals: u32, required: u32) -> u32 {
    match status {
        "draft" | "submitted" => required.saturating_sub(approvals),
        _ => 0,
    }
}

/// `GET /api/manual/experiments/{id}`: one record with its audit trail,
/// attachments and sign-offs, the sign-offs its type needs
/// (`signoffs_required`) and how many are still needed
/// (`signoffs_needed`). Records in an environment outside the principal's
/// scopes 404 (no existence leak across scopes).
pub async fn detail(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let detail = with_reader(&state.db_path, move |reader| {
        reader
            .manual_experiment_detail(&id)
            .map_err(|e| e.to_string())
    })
    .await?;
    let not_found = || crate::error::not_found("manual experiment not found");
    let Some(detail) = detail else {
        return Err(not_found());
    };
    if !principal.env_scopes.is_empty() {
        let env = detail.experiment["target_environment"].as_str();
        if !env.is_some_and(|e| principal.env_scopes.iter().any(|s| s == e)) {
            return Err(not_found());
        }
    }
    let (approvals, required) = signoff_counts(&state, &detail.experiment, &detail.signoffs);
    let status = detail.experiment["status"].as_str().unwrap_or_default();
    let needed = still_needed(status, approvals, required);
    let mut body = serde_json::to_value(detail).map_err(|e| internal(e.to_string()))?;
    body["signoffs_required"] = json!(required);
    body["signoffs_needed"] = json!(needed);
    Ok(Json(body))
}

/// `GET /api/manual/experiments/{id}/audit/verify`: check the record
/// again: its seal in its `hash_version`, the chain of its audit rows, its
/// reviewer and its sign-offs. `ok: false` lists each problem with the
/// table and the row it is in. Viewer, and 404 for an unknown record or
/// one outside the principal's scopes, as `GET /api/runs/{id}/audit/verify`.
pub async fn audit_verify(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let scopes = principal.env_scopes.clone();
    let lookup = id.clone();
    let verdict = with_reader(&state.db_path, move |reader| {
        if !scopes.is_empty() {
            let row = reader
                .manual_experiment_detail(&lookup)
                .map_err(|e| e.to_string())?;
            let env = row
                .as_ref()
                .and_then(|d| d.experiment["target_environment"].as_str());
            if !in_scopes(env, &scopes) {
                return Ok(None);
            }
        }
        reader
            .verify_manual_record(&lookup)
            .map_err(|e| e.to_string())
    })
    .await?;
    let Some(verdict) = verdict else {
        return Err(crate::error::not_found("manual experiment not found"));
    };
    let mut body = json!({"experiment_id": id, "chain_valid": verdict.ok});
    merge_verdict(&mut body, &verdict)?;
    Ok(Json(body))
}

/// Add the fields of `verdict` to a verify response.
pub(crate) fn merge_verdict(
    body: &mut Value,
    verdict: &tumult_lake::AuditVerdict,
) -> Result<(), ApiError> {
    let Value::Object(fields) =
        serde_json::to_value(verdict).map_err(|e| internal(e.to_string()))?
    else {
        return Err(internal("a verdict is not an object".into()));
    };
    if let Some(map) = body.as_object_mut() {
        map.extend(fields);
    }
    Ok(())
}

/// `PUT /api/manual/experiments/{id}` — replace a draft's content.
pub async fn update(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<ManualRecordRequest>,
) -> Result<Json<Value>, ApiError> {
    let changed_by =
        actor_or(&principal, req.entered_by.clone(), "entered_by").map_err(bad_request)?;
    check_environment(&principal, req.target_environment.as_deref())?;
    let mut subjects = named_plans([&req.plan_id]);
    subjects.push(Subject::Record(id.clone()));
    let new = req.into_new(changed_by.clone()).map_err(bad_request)?;
    exec_scoped(&state, &principal, subjects, move |w| {
        w.update_manual_draft(&id, &new, &changed_by)
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SubmitRequest {
    by: Option<String>,
    attestation: Option<String>,
}

/// `POST /api/manual/experiments/{id}/submit` — lock a draft for review.
pub async fn submit(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<SubmitRequest>,
) -> Result<Json<Value>, ApiError> {
    let by = actor_or(&principal, req.by.clone(), "by").map_err(bad_request)?;
    let resource_id = id.clone();
    exec_manual(&state, &principal, Some(&resource_id), move |w| {
        w.submit_manual(&id, req.attestation.as_deref(), &by)
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

/// The answer to a sign-off: where the record stands now.
fn signed(outcome: &SignOffOutcome) -> Json<Value> {
    Json(json!({
        "ok": true,
        "status": outcome.status,
        "signoffs_required": outcome.required,
        "signoffs_needed": still_needed(&outcome.status, outcome.approvals, outcome.required),
    }))
}

/// Sign record `id` off as `reviewer` on the single writer, with the
/// approvals the house parameters ask for its type.
async fn sign_off(
    state: &ApiState,
    principal: &Principal,
    id: String,
    reviewer: String,
    note: Option<String>,
    decision: SignOffDecision,
) -> Result<Json<Value>, ApiError> {
    let params = state.score_runtime().params();
    let resource_id = id.clone();
    let outcome = exec_manual(state, principal, Some(&resource_id), move |w| {
        let sign_off = SignOff {
            by: &reviewer,
            note: note.as_deref(),
            decision,
        };
        w.sign_off_manual(&id, &sign_off, |t| params.required_signoffs(t))
    })
    .await?;
    Ok(signed(&outcome))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifyRequest {
    reviewer: Option<String>,
    note: Option<String>,
}

/// `POST /api/manual/experiments/{id}/verify`: an approving sign-off. The
/// last approval the type needs verifies the record; the answer says the
/// status and how many sign-offs are still needed. The signer must not
/// have entered or changed the record, nor signed it off before (422).
pub async fn verify(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<VerifyRequest>,
) -> Result<Json<Value>, ApiError> {
    let reviewer = actor_or(&principal, req.reviewer.clone(), "reviewer").map_err(bad_request)?;
    sign_off(
        &state,
        &principal,
        id,
        reviewer,
        req.note,
        SignOffDecision::Approved,
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RejectRequest {
    reviewer: Option<String>,
    note: String,
}

/// `POST /api/manual/experiments/{id}/reject`: a rejecting sign-off. It
/// rejects the record whatever approvals it has. The note is mandatory.
pub async fn reject(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<RejectRequest>,
) -> Result<Json<Value>, ApiError> {
    let reviewer = actor_or(&principal, req.reviewer.clone(), "reviewer").map_err(bad_request)?;
    sign_off(
        &state,
        &principal,
        id,
        reviewer,
        Some(req.note),
        SignOffDecision::Rejected,
    )
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachmentRequest {
    kind: String,
    uri: String,
    label: Option<String>,
    added_by: Option<String>,
}

/// `POST /api/manual/experiments/{id}/attachments`: evidence for a record.
/// A link (`url` or `ticket`) is a JSON body, while the record is a draft
/// or submitted. A file is its bytes as an `application/octet-stream` body
/// with `?name=`, in any status (see [`crate::manual_files`]). Any other
/// content type is 415.
pub async fn attach(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    request: Request,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    match crate::manual_files::sent(request.headers()) {
        crate::manual_files::Sent::File => {
            return crate::manual_files::upload(state, principal, id, request).await;
        }
        crate::manual_files::Sent::Other => {
            return Err(crate::manual_files::not_a_link_or_a_file(request).await);
        }
        crate::manual_files::Sent::Link => {}
    }
    let Json(req) = Json::<AttachmentRequest>::from_request(request, &state)
        .await
        .map_err(|rejection| ApiError::from(rejection.into_response()))?;
    let kind = match req.kind.as_str() {
        "url" => AttachmentKind::Url,
        "ticket" => AttachmentKind::Ticket,
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!(
                    "attachment kind '{other}' not accepted as JSON (url|ticket); \
                     send a file as application/octet-stream with ?name=<file name>"
                )})),
            )
                .into_response()
                .into());
        }
    };
    let added_by = actor_or(&principal, req.added_by.clone(), "added_by").map_err(bad_request)?;
    let resource_id = id.clone();
    let attachment_id = exec_manual(&state, &principal, Some(&resource_id), move |w| {
        w.add_manual_attachment(&id, kind, &req.uri, req.label.as_deref(), None, &added_by)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(json!({"id": attachment_id}))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    label: Option<String>,
    records: Vec<ManualRecordRequest>,
}

/// `POST /api/manual/import` — bulk-create records as drafts in one batch.
/// Attestation is not bypassed: every record still needs submit → verify to
/// score.
pub async fn import(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<ImportRequest>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    let subjects = named_plans(req.records.iter().map(|r| &r.plan_id));
    let mut items = Vec::with_capacity(req.records.len());
    for record in req.records {
        check_environment(&principal, record.target_environment.as_deref())?;
        let entered_by =
            actor_or(&principal, record.entered_by.clone(), "entered_by").map_err(bad_request)?;
        items.push(record.into_new(entered_by).map_err(bad_request)?);
    }
    let label = req.label.clone();
    let (batch_id, ids) = exec_scoped(&state, &principal, subjects, move |w| {
        w.import_manual_drafts(&items, label)
    })
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"batch_id": batch_id, "ids": ids})),
    ))
}

pub(crate) fn check_environment(principal: &Principal, env: Option<&str>) -> Result<(), ApiError> {
    if !principal.env_allowed(env.unwrap_or_default()) {
        return Err(crate::error::forbidden(
            "environment is outside the principal's scopes",
        ));
    }
    Ok(())
}
