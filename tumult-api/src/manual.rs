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

use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_ingest::Batch;
use tumult_lake::{
    AttachmentKind, ExerciseType, ManualError, ManualOutcome, NewManualExperiment, Writer,
};

use crate::auth::Principal;
use crate::{internal, with_reader, ApiState};

/// The acting identity for a mutation: the authenticated principal's
/// username when auth is enabled, else the request-supplied field (required,
/// as before auth existed).
fn actor_or(
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

/// Map a lifecycle error to an HTTP response.
fn manual_error(err: &ManualError) -> Response {
    let status = match err {
        ManualError::Invalid(_) | ManualError::SelfReview => StatusCode::BAD_REQUEST,
        ManualError::NotFound(_) => StatusCode::NOT_FOUND,
        ManualError::WrongStatus { .. } => StatusCode::CONFLICT,
        ManualError::Store(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (status, Json(json!({"error": err.to_string()}))).into_response()
}

/// Run a typed manual-evidence mutation on the single writer and surface
/// the typed result (the `Batch::Exec` closure itself always "succeeds" so
/// the channel ack stays clean; the real outcome travels in `slot`).
async fn exec_manual<T>(
    state: &ApiState,
    f: impl FnOnce(&Writer) -> Result<T, ManualError> + Send + 'static,
) -> Result<T, Response>
where
    T: Send + 'static,
{
    let Some(ingest) = state.ingest_handle() else {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": "manual evidence writes are not wired (no ingest handle)"})),
        )
            .into_response());
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
        Some(Ok(v)) => Ok(v),
        Some(Err(e)) => Err(manual_error(&e)),
        None => Err(internal("manual operation did not run".into())),
    }
}

/// JSON body for create/update: content fields of a manual test record.
/// Enum-typed fields arrive as their lowercase string forms. `entered_by` is
/// ignored when auth is enabled (the principal's username wins); while auth
/// is open it is required.
#[derive(Debug, Deserialize)]
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
}

impl ManualRecordRequest {
    fn into_new(self, entered_by: String) -> Result<NewManualExperiment, ManualError> {
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
        })
    }
}

fn bad_request(err: ManualError) -> Response {
    manual_error(&err)
}

/// `POST /api/manual/experiments` — create a draft record.
pub async fn create(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Json(req): Json<ManualRecordRequest>,
) -> Result<(StatusCode, Json<Value>), Response> {
    let entered_by =
        actor_or(&principal, req.entered_by.clone(), "entered_by").map_err(bad_request)?;
    let new = req.into_new(entered_by).map_err(bad_request)?;
    let id = exec_manual(&state, move |w| w.create_manual_draft(&new)).await?;
    Ok((StatusCode::CREATED, Json(json!({"id": id}))))
}

#[derive(Deserialize)]
pub struct ListParams {
    status: Option<String>,
}

/// `GET /api/manual/experiments?status=` — list records (newest first).
pub async fn list(
    State(state): State<ApiState>,
    Query(params): Query<ListParams>,
) -> Result<Json<Value>, Response> {
    let status = params.status.clone();
    let rows = with_reader(&state.db_path, move |reader| {
        reader
            .manual_experiments(status.as_deref())
            .map_err(|e| e.to_string())
    })
    .await?;
    Ok(Json(json!({"records": rows})))
}

/// `GET /api/manual/experiments/{id}` — one record with audit + attachments.
/// Records in an environment outside the principal's scopes 404 (no
/// existence leak across scopes).
pub async fn detail(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    let detail = with_reader(&state.db_path, move |reader| {
        reader
            .manual_experiment_detail(&id)
            .map_err(|e| e.to_string())
    })
    .await?;
    let not_found = || {
        (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "manual experiment not found"})),
        )
            .into_response()
    };
    let Some(detail) = detail else {
        return Err(not_found());
    };
    if !principal.env_scopes.is_empty() {
        let env = detail.experiment["target_environment"].as_str();
        if !env.is_some_and(|e| principal.env_scopes.iter().any(|s| s == e)) {
            return Err(not_found());
        }
    }
    Ok(Json(
        serde_json::to_value(detail).map_err(|e| internal(e.to_string()))?,
    ))
}

/// `PUT /api/manual/experiments/{id}` — replace a draft's content.
pub async fn update(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<ManualRecordRequest>,
) -> Result<Json<Value>, Response> {
    let changed_by =
        actor_or(&principal, req.entered_by.clone(), "entered_by").map_err(bad_request)?;
    let new = req.into_new(changed_by.clone()).map_err(bad_request)?;
    exec_manual(&state, move |w| {
        w.update_manual_draft(&id, &new, &changed_by)
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
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
) -> Result<Json<Value>, Response> {
    let by = actor_or(&principal, req.by.clone(), "by").map_err(bad_request)?;
    exec_manual(&state, move |w| {
        w.submit_manual(&id, req.attestation.as_deref(), &by)
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct VerifyRequest {
    reviewer: Option<String>,
    note: Option<String>,
}

/// `POST /api/manual/experiments/{id}/verify` — reviewer ≠ enterer.
pub async fn verify(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<VerifyRequest>,
) -> Result<Json<Value>, Response> {
    let reviewer = actor_or(&principal, req.reviewer.clone(), "reviewer").map_err(bad_request)?;
    exec_manual(&state, move |w| {
        w.verify_manual(&id, &reviewer, req.note.as_deref())
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct RejectRequest {
    reviewer: Option<String>,
    note: String,
}

/// `POST /api/manual/experiments/{id}/reject` — note is mandatory.
pub async fn reject(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<RejectRequest>,
) -> Result<Json<Value>, Response> {
    let reviewer = actor_or(&principal, req.reviewer.clone(), "reviewer").map_err(bad_request)?;
    exec_manual(&state, move |w| w.reject_manual(&id, &reviewer, &req.note)).await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct AttachmentRequest {
    kind: String,
    uri: String,
    label: Option<String>,
    added_by: Option<String>,
}

/// `POST /api/manual/experiments/{id}/attachments` — external evidence
/// links. Only `url` and `ticket` are accepted: there is no file storage.
pub async fn attach(
    State(state): State<ApiState>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    Json(req): Json<AttachmentRequest>,
) -> Result<(StatusCode, Json<Value>), Response> {
    let kind = match req.kind.as_str() {
        "url" => AttachmentKind::Url,
        "ticket" => AttachmentKind::Ticket,
        other => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({"error": format!(
                    "attachment kind '{other}' not accepted (url|ticket only; no file storage)"
                )})),
            )
                .into_response());
        }
    };
    let added_by = actor_or(&principal, req.added_by.clone(), "added_by").map_err(bad_request)?;
    let attachment_id = exec_manual(&state, move |w| {
        w.add_manual_attachment(&id, kind, &req.uri, req.label.as_deref(), None, &added_by)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(json!({"id": attachment_id}))))
}

#[derive(Deserialize)]
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
) -> Result<(StatusCode, Json<Value>), Response> {
    let mut items = Vec::with_capacity(req.records.len());
    for record in req.records {
        let entered_by =
            actor_or(&principal, record.entered_by.clone(), "entered_by").map_err(bad_request)?;
        items.push(record.into_new(entered_by).map_err(bad_request)?);
    }
    let label = req.label.clone();
    let (batch_id, ids) =
        exec_manual(&state, move |w| w.import_manual_drafts(&items, label)).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"batch_id": batch_id, "ids": ids})),
    ))
}
