//! Manual evidence endpoints (`/api/manual/*`).
//!
//! Mutations ride the daemon's single-writer channel via
//! [`tumult_ingest::Batch::Exec`] — the API never opens a write connection
//! of its own. There is no auth yet: callers pass a plain "acting as" user
//! string (`entered_by` / `by` / `reviewer`); the store enforces the
//! lifecycle rules (draft mutability, attestation on submit, reviewer ≠
//! enterer) regardless.

use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use tumult_ingest::Batch;
use tumult_lake::{
    AttachmentKind, ExerciseType, ManualError, ManualOutcome, NewManualExperiment, Writer,
};

use crate::{internal, with_reader, ApiState};

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
/// Enum-typed fields arrive as their lowercase string forms.
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
    entered_by: String,
    attestation: String,
    renewal_due_ns: Option<i64>,
    #[serde(default)]
    framework_refs: Vec<String>,
}

impl ManualRecordRequest {
    fn into_new(self) -> Result<NewManualExperiment, ManualError> {
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
            entered_by: self.entered_by,
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
    Json(req): Json<ManualRecordRequest>,
) -> Result<(StatusCode, Json<Value>), Response> {
    let new = req.into_new().map_err(bad_request)?;
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
pub async fn detail(
    State(state): State<ApiState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Response> {
    let detail = with_reader(&state.db_path, move |reader| {
        reader
            .manual_experiment_detail(&id)
            .map_err(|e| e.to_string())
    })
    .await?;
    let Some(detail) = detail else {
        return Err((
            StatusCode::NOT_FOUND,
            Json(json!({"error": "manual experiment not found"})),
        )
            .into_response());
    };
    Ok(Json(
        serde_json::to_value(detail).map_err(|e| internal(e.to_string()))?,
    ))
}

/// `PUT /api/manual/experiments/{id}` — replace a draft's content.
pub async fn update(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(req): Json<ManualRecordRequest>,
) -> Result<Json<Value>, Response> {
    let changed_by = req.entered_by.clone();
    let new = req.into_new().map_err(bad_request)?;
    exec_manual(&state, move |w| {
        w.update_manual_draft(&id, &new, &changed_by)
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct SubmitRequest {
    by: String,
    attestation: Option<String>,
}

/// `POST /api/manual/experiments/{id}/submit` — lock a draft for review.
pub async fn submit(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(req): Json<SubmitRequest>,
) -> Result<Json<Value>, Response> {
    exec_manual(&state, move |w| {
        w.submit_manual(&id, req.attestation.as_deref(), &req.by)
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct VerifyRequest {
    reviewer: String,
    note: Option<String>,
}

/// `POST /api/manual/experiments/{id}/verify` — reviewer ≠ enterer.
pub async fn verify(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(req): Json<VerifyRequest>,
) -> Result<Json<Value>, Response> {
    exec_manual(&state, move |w| {
        w.verify_manual(&id, &req.reviewer, req.note.as_deref())
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct RejectRequest {
    reviewer: String,
    note: String,
}

/// `POST /api/manual/experiments/{id}/reject` — note is mandatory.
pub async fn reject(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    Json(req): Json<RejectRequest>,
) -> Result<Json<Value>, Response> {
    exec_manual(&state, move |w| {
        w.reject_manual(&id, &req.reviewer, &req.note)
    })
    .await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
pub struct AttachmentRequest {
    kind: String,
    uri: String,
    label: Option<String>,
    added_by: String,
}

/// `POST /api/manual/experiments/{id}/attachments` — external evidence
/// links. Only `url` and `ticket` are accepted: there is no file storage.
pub async fn attach(
    State(state): State<ApiState>,
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
    let attachment_id = exec_manual(&state, move |w| {
        w.add_manual_attachment(
            &id,
            kind,
            &req.uri,
            req.label.as_deref(),
            None,
            &req.added_by,
        )
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
    Json(req): Json<ImportRequest>,
) -> Result<(StatusCode, Json<Value>), Response> {
    let mut items = Vec::with_capacity(req.records.len());
    for record in req.records {
        items.push(record.into_new().map_err(bad_request)?);
    }
    let label = req.label.clone();
    let (batch_id, ids) =
        exec_manual(&state, move |w| w.import_manual_drafts(&items, label)).await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({"batch_id": batch_id, "ids": ids})),
    ))
}
