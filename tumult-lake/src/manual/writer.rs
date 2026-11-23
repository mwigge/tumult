//! The write side of the manual-evidence lifecycle: draft creation and
//! replacement, submit/verify/reject, attachments, and bulk import — every
//! mutation hash-chained into the audit table.

use duckdb::{params, Connection};

use super::chain::{self, insert_signoff, NewSignoff, Trail};
use super::files::StagedFile;
use super::hash::{fetch_row, now_ns, reseal, status_of, ulid};
use super::plans::{check_tested, fulfil, usable_plan, Tested};
use super::{
    AttachmentKind, ExerciseDetail, LinkKind, ManualError, NewManualExperiment, SignOff,
    SignOffDecision, SignOffOutcome, StepResult,
};
use crate::{with_tx, StoreError, Writer};

/// The statements that remove the steps, actions and links of a record.
const DELETE_CHILDREN: [&str; 3] = [
    "DELETE FROM manual_exercise_steps WHERE experiment_id = ?",
    "DELETE FROM manual_action_items WHERE experiment_id = ?",
    "DELETE FROM manual_experiment_links WHERE experiment_id = ?",
];

/// The participants as the JSON list the `participants` column holds.
fn participants_json(detail: &ExerciseDetail) -> String {
    let list: Vec<serde_json::Value> = detail
        .participants
        .iter()
        .map(|p| serde_json::json!({"name": p.name, "role": p.role}))
        .collect();
    serde_json::Value::from(list).to_string()
}

/// Write the steps, actions and links of record `id`, numbered from 1 in
/// the order given. Actions start `open` (no transaction; caller wraps).
fn insert_children(
    conn: &Connection,
    id: &str,
    detail: &ExerciseDetail,
) -> Result<(), ManualError> {
    for (ordinal, step) in (1_i64..).zip(&detail.steps) {
        conn.execute(
            "INSERT INTO manual_exercise_steps (
                experiment_id, ordinal, title, actor, started_at_ns, ended_at_ns, result, note
             ) VALUES (?,?,?,?,?,?,?,?)",
            params![
                id,
                ordinal,
                step.title,
                step.actor,
                step.started_at_ns,
                step.ended_at_ns,
                step.result.map(StepResult::as_str),
                step.note,
            ],
        )?;
    }
    for (ordinal, action) in (1_i64..).zip(&detail.actions) {
        conn.execute(
            "INSERT INTO manual_action_items (
                id, experiment_id, ordinal, title, owner, due_at_ns, ticket
             ) VALUES (?,?,?,?,?,?,?)",
            params![
                ulid(),
                id,
                ordinal,
                action.title,
                action.owner,
                action.due_at_ns,
                action.ticket,
            ],
        )?;
    }
    for (ordinal, link) in (1_i64..).zip(&detail.links) {
        conn.execute(
            "INSERT INTO manual_experiment_links (
                experiment_id, ordinal, kind, linked_id, label
             ) VALUES (?,?,?,?,?)",
            params![id, ordinal, link.kind.as_str(), link.linked_id, link.label],
        )?;
    }
    Ok(())
}

/// Refuse the plan `new` names unless the record can fulfil it: the plan
/// is planned, of the record's type, and plans what the record tests.
fn check_plan(conn: &Connection, new: &NewManualExperiment) -> Result<(), ManualError> {
    let Some(plan_id) = &new.exercise.plan_id else {
        return Ok(());
    };
    let row = usable_plan(conn, plan_id, new.exercise_type.as_str())?;
    let tested = Tested {
        target_system: new.target_system.as_deref(),
        target_environment: new.target_environment.as_deref(),
        functions: new
            .exercise
            .links
            .iter()
            .filter(|l| l.kind == LinkKind::Function)
            .map(|l| l.linked_id.as_str())
            .collect(),
    };
    check_tested(&row, plan_id, &tested)
}

/// Insert the experiment row and its child rows, then seal it (no
/// transaction; caller wraps). The row is sealed from what was stored, as
/// every later write seals it.
fn insert_draft(
    conn: &Connection,
    new: &NewManualExperiment,
    batch_id: Option<&str>,
) -> Result<(String, String), ManualError> {
    check_plan(conn, new)?;
    let id = ulid();
    let detail = &new.exercise;
    conn.execute(
        "INSERT INTO manual_experiments (
            id, experiment_name, exercise_type, executed_at_ns, hypothesis, method,
            outcome_status, hypothesis_met, findings, action_items,
            target_system, target_environment, blast_radius,
            recovery_time_s, duration_s, origin,
            entered_by, entered_at_ns, attestation, status,
            renewal_due_ns, framework_refs, batch_id, content_hash,
            rto_target_s, rpo_target_s, rpo_measured_s, data_verified, participants,
            plan_id
         ) VALUES (?,?,?,?,?,?,?,?,?,CAST(? AS JSON),?,?,?,?,?,?,
                   ?,?,?,?,?, CAST(? AS VARCHAR[]),?,?,?,?,?,?,CAST(? AS JSON),?)",
        params![
            id,
            new.experiment_name,
            new.exercise_type.as_str(),
            new.executed_at_ns,
            new.hypothesis,
            new.method,
            new.outcome.as_str(),
            new.hypothesis_met,
            new.findings,
            serde_json::to_string(&new.action_items).unwrap_or_else(|_| "[]".into()),
            new.target_system,
            new.target_environment,
            new.blast_radius,
            new.recovery_time_s,
            new.duration_s,
            "manual",
            new.entered_by,
            now_ns(),
            new.attestation,
            "draft",
            new.renewal_due_ns,
            serde_json::to_string(&new.framework_refs).unwrap_or_else(|_| "[]".into()),
            batch_id,
            // Replaced by the seal below, in the same transaction.
            "",
            detail.rto_target_s,
            detail.rpo_target_s,
            detail.rpo_measured_s,
            detail.data_verified,
            participants_json(detail),
            detail.plan_id,
        ],
    )?;
    insert_children(conn, &id, detail)?;
    let hash = reseal(conn, &id)?;
    Ok((id, hash))
}

/// Append one audit row of record `experiment_id` (no transaction;
/// caller wraps).
fn append_audit(
    conn: &Connection,
    experiment_id: &str,
    changed_by: &str,
    action: &str,
    diff: &serde_json::Value,
    prev_hash: Option<&str>,
    new_hash: &str,
) -> Result<(), ManualError> {
    chain::append_audit(
        conn,
        Trail::Record,
        experiment_id,
        changed_by,
        action,
        diff,
        prev_hash,
        new_hash,
    )
}

impl Writer {
    /// Create a manual test record as a `draft`. Returns the new id.
    ///
    /// # Errors
    /// Returns `Invalid` when required fields are empty, `Store` on write failure.
    pub fn create_manual_draft(&self, new: &NewManualExperiment) -> Result<String, ManualError> {
        new.validate()?;
        with_tx(&self.conn, || {
            let (id, hash) = insert_draft(&self.conn, new, None)?;
            append_audit(
                &self.conn,
                &id,
                &new.entered_by,
                "create",
                &serde_json::json!({
                    "experiment_name": new.experiment_name,
                    "exercise_type": new.exercise_type.as_str(),
                    "outcome_status": new.outcome.as_str(),
                }),
                None,
                &hash,
            )?;
            Ok(id)
        })
    }

    /// Replace the content of a draft (PUT semantics). `entered_by` and
    /// `entered_at_ns` are preserved from the original record.
    ///
    /// # Errors
    /// Returns `NotFound`, `WrongStatus` when the record is no longer a
    /// draft, or `Invalid` for empty required fields.
    pub fn update_manual_draft(
        &self,
        id: &str,
        new: &NewManualExperiment,
        changed_by: &str,
    ) -> Result<(), ManualError> {
        new.validate()?;
        with_tx(&self.conn, || {
            let row =
                fetch_row(&self.conn, id)?.ok_or_else(|| ManualError::NotFound(id.to_string()))?;
            let status = status_of(&row);
            if status != "draft" {
                return Err(ManualError::WrongStatus {
                    status,
                    action: "edit".into(),
                });
            }
            let prev_hash = row
                .get("content_hash")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let entered_by = row
                .get("entered_by")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            check_plan(&self.conn, new)?;
            self.conn.execute(
                "UPDATE manual_experiments SET
                    experiment_name = ?, exercise_type = ?, executed_at_ns = ?,
                    hypothesis = ?, method = ?, outcome_status = ?,
                    hypothesis_met = ?, findings = ?, action_items = CAST(? AS JSON),
                    target_system = ?, target_environment = ?, blast_radius = ?,
                    recovery_time_s = ?, duration_s = ?, attestation = ?,
                    renewal_due_ns = ?, framework_refs = CAST(? AS VARCHAR[]),
                    rto_target_s = ?, rpo_target_s = ?, rpo_measured_s = ?,
                    data_verified = ?, participants = CAST(? AS JSON), plan_id = ?
                 WHERE id = ?",
                params![
                    new.experiment_name,
                    new.exercise_type.as_str(),
                    new.executed_at_ns,
                    new.hypothesis,
                    new.method,
                    new.outcome.as_str(),
                    new.hypothesis_met,
                    new.findings,
                    serde_json::to_string(&new.action_items).unwrap_or_else(|_| "[]".into()),
                    new.target_system,
                    new.target_environment,
                    new.blast_radius,
                    new.recovery_time_s,
                    new.duration_s,
                    new.attestation,
                    new.renewal_due_ns,
                    serde_json::to_string(&new.framework_refs).unwrap_or_else(|_| "[]".into()),
                    new.exercise.rto_target_s,
                    new.exercise.rpo_target_s,
                    new.exercise.rpo_measured_s,
                    new.exercise.data_verified,
                    participants_json(&new.exercise),
                    new.exercise.plan_id,
                    id,
                ],
            )?;
            for sql in DELETE_CHILDREN {
                self.conn.execute(sql, params![id])?;
            }
            insert_children(&self.conn, id, &new.exercise)?;
            let hash = reseal(&self.conn, id)?;
            append_audit(
                &self.conn,
                id,
                changed_by,
                "edit",
                &serde_json::json!({
                    "replaced_by": entered_by,
                    "fields": [
                        "experiment_name","exercise_type","executed_at_ns","hypothesis",
                        "method","outcome_status","hypothesis_met","findings",
                        "action_items","target_system","target_environment","blast_radius",
                        "recovery_time_s","duration_s","attestation","renewal_due_ns",
                        "framework_refs","rto_target_s","rpo_target_s","rpo_measured_s",
                        "data_verified","participants","steps","actions","links",
                        "plan_id"
                    ],
                }),
                Some(&prev_hash),
                &hash,
            )?;
            Ok(())
        })
    }

    /// Move a draft to `submitted`, locking it. A fresh attestation text may
    /// be supplied; otherwise the existing one must be non-empty.
    ///
    /// # Errors
    /// Returns `NotFound`, `WrongStatus`, or `Invalid` when no attestation
    /// text is present.
    pub fn submit_manual(
        &self,
        id: &str,
        attestation: Option<&str>,
        by: &str,
    ) -> Result<(), ManualError> {
        with_tx(&self.conn, || {
            let row =
                fetch_row(&self.conn, id)?.ok_or_else(|| ManualError::NotFound(id.to_string()))?;
            let status = status_of(&row);
            if status != "draft" {
                return Err(ManualError::WrongStatus {
                    status,
                    action: "submit".into(),
                });
            }
            let prev_hash = row
                .get("content_hash")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let effective = attestation
                .map(str::to_owned)
                .or_else(|| {
                    row.get("attestation")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            if effective.trim().is_empty() {
                return Err(ManualError::Invalid(
                    "submit requires a non-empty attestation".into(),
                ));
            }
            self.conn.execute(
                "UPDATE manual_experiments SET status = 'submitted', attestation = ? \
                 WHERE id = ?",
                params![effective, id],
            )?;
            let hash = reseal(&self.conn, id)?;
            append_audit(
                &self.conn,
                id,
                by,
                "submit",
                &serde_json::json!({"from_status": "draft", "to_status": "submitted"}),
                Some(&prev_hash),
                &hash,
            )?;
            Ok(())
        })
    }

    /// Verify a submitted record that needs one approval. The reviewer must
    /// differ from the person who entered, edited, submitted or attached
    /// its evidence (segregation of duties).
    ///
    /// # Errors
    /// As [`Self::sign_off_manual`].
    pub fn verify_manual(
        &self,
        id: &str,
        reviewer: &str,
        note: Option<&str>,
    ) -> Result<(), ManualError> {
        let sign_off = SignOff {
            by: reviewer,
            note,
            decision: SignOffDecision::Approved,
        };
        self.sign_off_manual(id, &sign_off, |_| 1).map(|_| ())
    }

    /// Reject a submitted record (a review note is mandatory). The reviewer
    /// must not have created, edited, submitted or attached its evidence.
    ///
    /// # Errors
    /// As [`Self::sign_off_manual`].
    pub fn reject_manual(&self, id: &str, reviewer: &str, note: &str) -> Result<(), ManualError> {
        let sign_off = SignOff {
            by: reviewer,
            note: Some(note),
            decision: SignOffDecision::Rejected,
        };
        self.sign_off_manual(id, &sign_off, |_| 1).map(|_| ())
    }

    /// Sign off a submitted record. `required` gives the approvals an
    /// exercise type needs (at least one). An approval that leaves approvals
    /// missing keeps the record submitted; the last one verifies it and
    /// fulfils its plan. A reject rejects it. Each sign-off is a
    /// `manual_signoffs` row that signs the seal of the record and links the
    /// sign-off before it, and an audit row.
    ///
    /// # Errors
    /// Returns `NotFound`, `WrongStatus` (not submitted), `SelfReview`
    /// (the signer entered, edited, submitted or attached to the record),
    /// `AlreadySigned`, or `Invalid` for an empty signer or a reject
    /// without a note.
    pub fn sign_off_manual(
        &self,
        id: &str,
        sign_off: &SignOff<'_>,
        required: impl Fn(&str) -> u32,
    ) -> Result<SignOffOutcome, ManualError> {
        let approve = sign_off.decision == SignOffDecision::Approved;
        if !approve && sign_off.note.is_none_or(|n| n.trim().is_empty()) {
            return Err(ManualError::Invalid(
                "reject requires a non-empty review note".into(),
            ));
        }
        let action = if approve { "verify" } else { "reject" };
        with_tx(&self.conn, || {
            let row =
                fetch_row(&self.conn, id)?.ok_or_else(|| ManualError::NotFound(id.to_string()))?;
            let status = status_of(&row);
            if status != "submitted" {
                return Err(ManualError::WrongStatus {
                    status,
                    action: action.into(),
                });
            }
            self.may_sign(id, &row, sign_off.by)?;
            let text = |k: &str| row.get(k).and_then(serde_json::Value::as_str);
            let seal = text("content_hash").unwrap_or_default().to_string();
            let required = required(text("exercise_type").unwrap_or_default()).max(1);
            let approved_before: i64 = self.conn.query_row(
                "SELECT COUNT(*) FROM manual_signoffs WHERE subject_kind = 'record' \
                 AND subject_id = ? AND decision = 'approved'",
                params![id],
                |r| r.get(0),
            )?;
            let approvals = u32::try_from(approved_before)
                .unwrap_or(u32::MAX)
                .saturating_add(u32::from(approve));
            let signoff_id = insert_signoff(
                &self.conn,
                &NewSignoff {
                    subject_id: id,
                    subject_kind: "record",
                    role: "reviewer",
                    signed_by: sign_off.by,
                    decision: sign_off.decision.as_str(),
                    note: sign_off.note,
                    signed_hash: &seal,
                },
            )?;
            if approve && approvals < required {
                append_audit(
                    &self.conn,
                    id,
                    sign_off.by,
                    "signoff",
                    &serde_json::json!({
                        "signoff_id": signoff_id,
                        "decision": sign_off.decision.as_str(),
                        "approvals": approvals,
                        "required": required,
                        "note": sign_off.note,
                    }),
                    Some(&seal),
                    &seal,
                )?;
                return Ok(SignOffOutcome {
                    status: status.clone(),
                    approvals,
                    required,
                });
            }
            let to_status = if approve { "verified" } else { "rejected" };
            self.conn.execute(
                "UPDATE manual_experiments SET status = ?, reviewed_by = ?, \
                 reviewed_at_ns = ?, review_note = ? WHERE id = ?",
                params![to_status, sign_off.by, now_ns(), sign_off.note, id],
            )?;
            let hash = reseal(&self.conn, id)?;
            append_audit(
                &self.conn,
                id,
                sign_off.by,
                action,
                &serde_json::json!({
                    "from_status": "submitted",
                    "to_status": to_status,
                    "note": sign_off.note,
                    "signoff_id": signoff_id,
                }),
                Some(&seal),
                &hash,
            )?;
            // A verified record fulfils the plan it names; a rejected one
            // leaves the plan as it was.
            if let Some(plan_id) = text("plan_id").filter(|_| approve) {
                fulfil(&self.conn, plan_id, &row, sign_off.by)?;
            }
            Ok(SignOffOutcome {
                status: to_status.into(),
                approvals,
                required,
            })
        })
    }

    /// Refuse `by` as a signer of record `id` (stored as `row`) when the
    /// name is empty, when they entered, edited, submitted or attached to
    /// the record, or when they signed it off before.
    fn may_sign(&self, id: &str, row: &serde_json::Value, by: &str) -> Result<(), ManualError> {
        if by.trim().is_empty() {
            return Err(ManualError::Invalid("reviewer must not be empty".into()));
        }
        let entered_by = row
            .get("entered_by")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let contributed: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM manual_experiment_audit WHERE experiment_id = ? AND changed_by = ? AND action IN ('create', 'edit', 'submit', 'attach')",
            params![id, by], |row| row.get(0),
        )?;
        if by == entered_by || contributed > 0 {
            return Err(ManualError::SelfReview);
        }
        let signed: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM manual_signoffs WHERE subject_kind = 'record' \
             AND subject_id = ? AND signed_by = ?",
            params![id, by],
            |row| row.get(0),
        )?;
        if signed > 0 {
            return Err(ManualError::AlreadySigned(by.to_string()));
        }
        Ok(())
    }

    /// Attach an external evidence link to a draft or submitted record.
    /// Returns the attachment id. A file is attached with
    /// [`Self::add_manual_file`], which stores it.
    ///
    /// # Errors
    /// Returns `NotFound`, `WrongStatus` (verified/rejected records are
    /// locked), or `Invalid` for an empty URI or the kind `file`.
    pub fn add_manual_attachment(
        &self,
        experiment_id: &str,
        kind: AttachmentKind,
        uri: &str,
        label: Option<&str>,
        file_hash: Option<&str>,
        added_by: &str,
    ) -> Result<String, ManualError> {
        if kind == AttachmentKind::File {
            return Err(ManualError::Invalid(
                "a file is attached with its bytes, not as a link".into(),
            ));
        }
        if uri.trim().is_empty() {
            return Err(ManualError::Invalid(
                "attachment uri must not be empty".into(),
            ));
        }
        with_tx(&self.conn, || {
            let row = fetch_row(&self.conn, experiment_id)?
                .ok_or_else(|| ManualError::NotFound(experiment_id.to_string()))?;
            let status = status_of(&row);
            if status != "draft" && status != "submitted" {
                return Err(ManualError::WrongStatus {
                    status,
                    action: "attach".into(),
                });
            }
            let content_hash = row
                .get("content_hash")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let attachment_id = ulid();
            self.conn.execute(
                "INSERT INTO evidence_attachments (
                    id, experiment_id, kind, uri, label, file_hash, added_by, added_at_ns
                 ) VALUES (?,?,?,?,?,?,?,?)",
                params![
                    attachment_id,
                    experiment_id,
                    kind.as_str(),
                    uri,
                    label,
                    file_hash,
                    added_by,
                    now_ns(),
                ],
            )?;
            // Attachments do not alter the content hash; the audit row still
            // chains prev == new so the sequence stays verifiable.
            append_audit(
                &self.conn,
                experiment_id,
                added_by,
                "attach",
                &serde_json::json!({
                    "attachment_id": attachment_id,
                    "kind": kind.as_str(),
                    "uri": uri,
                    "label": label,
                }),
                Some(&content_hash),
                &content_hash,
            )?;
            Ok(attachment_id)
        })
    }

    /// Store `file` by its hash and attach it to record `experiment_id`, in
    /// any status: the attach audit row names the attachment and the hash,
    /// and the seal of the record stays as it was. Returns the attachment
    /// id. A file stored here and not there before is removed again when
    /// the row cannot be written.
    ///
    /// # Errors
    /// Returns `NotFound`, `Invalid` for an empty `added_by`, or `Store`
    /// when the file or the row cannot be written.
    pub fn add_manual_file(
        &self,
        experiment_id: &str,
        mut file: StagedFile,
        added_by: &str,
    ) -> Result<String, ManualError> {
        if added_by.trim().is_empty() {
            return Err(ManualError::Invalid("'added_by' must not be empty".into()));
        }
        let placed = file.place().map_err(StoreError::from)?;
        let stored = with_tx(&self.conn, || {
            let row = fetch_row(&self.conn, experiment_id)?
                .ok_or_else(|| ManualError::NotFound(experiment_id.to_string()))?;
            let seal = row
                .get("content_hash")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let attachment_id = ulid();
            // Where the daemon serves the file, so a link to the attachment
            // opens it.
            let uri =
                format!("/api/manual/experiments/{experiment_id}/attachments/{attachment_id}/file");
            let size = i64::try_from(file.size_bytes()).unwrap_or(i64::MAX);
            self.conn.execute(
                "INSERT INTO evidence_attachments (
                    id, experiment_id, kind, uri, label, file_hash, added_by, added_at_ns,
                    media_type, size_bytes
                 ) VALUES (?,?,?,?,?,?,?,?,?,?)",
                params![
                    attachment_id,
                    experiment_id,
                    AttachmentKind::File.as_str(),
                    uri,
                    file.name(),
                    file.hash(),
                    added_by,
                    now_ns(),
                    file.media_type(),
                    size,
                ],
            )?;
            append_audit(
                &self.conn,
                experiment_id,
                added_by,
                "attach",
                &serde_json::json!({
                    "attachment_id": attachment_id,
                    "kind": AttachmentKind::File.as_str(),
                    "uri": uri,
                    "label": file.name(),
                    "file_hash": file.hash(),
                    "media_type": file.media_type(),
                    "size_bytes": size,
                }),
                Some(&seal),
                &seal,
            )?;
            Ok(attachment_id)
        });
        if stored.is_err() && placed.new {
            if let Err(e) = std::fs::remove_file(&placed.path) {
                tracing::warn!(path = %placed.path.display(), error = %e, "could not remove a file whose row was not written");
            }
        }
        stored
    }

    /// Bulk-import records as drafts in one transaction, recording an
    /// `import_batches` row. Attestation is NOT bypassed: every item carries
    /// its own attestation text and still needs the submit → verify path to
    /// score. Returns `(batch_id, experiment_ids)`.
    ///
    /// # Errors
    /// Returns `Invalid` for any item failing validation (the whole batch
    /// rolls back), or `Store` on write failure.
    pub fn import_manual_drafts(
        &self,
        items: &[NewManualExperiment],
        label: Option<String>,
    ) -> Result<(String, Vec<String>), ManualError> {
        if items.is_empty() {
            return Err(ManualError::Invalid(
                "import requires at least one record".into(),
            ));
        }
        for item in items {
            item.validate()?;
        }
        with_tx(&self.conn, || {
            let batch_id = format!("manual-import-{}", ulid());
            self.conn.execute(
                "INSERT INTO import_batches (id, source, imported_at_ns, rows, label) \
                 VALUES (?,?,?,?,?)",
                params![
                    batch_id,
                    "manual-api",
                    now_ns(),
                    i32::try_from(items.len()).unwrap_or(i32::MAX),
                    label,
                ],
            )?;
            let mut ids = Vec::with_capacity(items.len());
            for item in items {
                let (id, hash) = insert_draft(&self.conn, item, Some(&batch_id))?;
                append_audit(
                    &self.conn,
                    &id,
                    &item.entered_by,
                    "create",
                    &serde_json::json!({
                        "experiment_name": item.experiment_name,
                        "batch_id": batch_id,
                        "via": "import",
                    }),
                    None,
                    &hash,
                )?;
                ids.push(id);
            }
            Ok((batch_id, ids))
        })
    }
}
