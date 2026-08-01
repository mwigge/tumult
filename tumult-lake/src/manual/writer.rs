//! The write side of the manual-evidence lifecycle: draft creation and
//! replacement, submit/verify/reject, attachments, and bulk import — every
//! mutation hash-chained into the audit table.

use duckdb::{params, Connection};

use super::hash::{content_hash, fetch_row, now_ns, rehash, status_of, ulid, CanonicalContent};
use super::{AttachmentKind, ManualError, NewManualExperiment};
use crate::{with_tx, Writer};

/// Insert the experiment row (no transaction; caller wraps).
fn insert_draft(
    conn: &Connection,
    new: &NewManualExperiment,
    batch_id: Option<&str>,
) -> Result<(String, String), ManualError> {
    let id = ulid();
    let canonical = CanonicalContent {
        experiment_name: &new.experiment_name,
        exercise_type: new.exercise_type.as_str(),
        executed_at_ns: new.executed_at_ns,
        hypothesis: &new.hypothesis,
        method: &new.method,
        outcome_status: new.outcome.as_str(),
        hypothesis_met: new.hypothesis_met,
        findings: new.findings.as_deref(),
        action_items: &new.action_items,
        target_system: new.target_system.as_deref(),
        target_environment: new.target_environment.as_deref(),
        blast_radius: new.blast_radius.as_deref(),
        recovery_time_s: new.recovery_time_s,
        duration_s: new.duration_s,
        entered_by: &new.entered_by,
        attestation: &new.attestation,
        renewal_due_ns: new.renewal_due_ns,
        framework_refs: &new.framework_refs,
        status: "draft",
    };
    let hash = content_hash(&canonical);
    conn.execute(
        "INSERT INTO manual_experiments (
            id, experiment_name, exercise_type, executed_at_ns, hypothesis, method,
            outcome_status, hypothesis_met, findings, action_items,
            target_system, target_environment, blast_radius,
            recovery_time_s, duration_s, origin,
            entered_by, entered_at_ns, attestation, status,
            renewal_due_ns, framework_refs, batch_id, content_hash
         ) VALUES (?,?,?,?,?,?,?,?,?,CAST(? AS JSON),?,?,?,?,?,?,
                   ?,?,?,?,?, CAST(? AS VARCHAR[]),?,?)",
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
            hash,
        ],
    )?;
    Ok((id, hash))
}

/// Append one audit row (no transaction; caller wraps).
fn append_audit(
    conn: &Connection,
    experiment_id: &str,
    changed_by: &str,
    action: &str,
    diff: &serde_json::Value,
    prev_hash: Option<&str>,
    new_hash: &str,
) -> Result<(), ManualError> {
    conn.execute(
        "INSERT INTO manual_experiment_audit (
            id, experiment_id, changed_by, changed_at_ns, action, diff, prev_hash, new_hash
         ) VALUES (?,?,?,?,?,CAST(? AS JSON),?,?)",
        params![
            ulid(),
            experiment_id,
            changed_by,
            now_ns(),
            action,
            diff.to_string(),
            prev_hash,
            new_hash,
        ],
    )?;
    Ok(())
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
            self.conn.execute(
                "UPDATE manual_experiments SET
                    experiment_name = ?, exercise_type = ?, executed_at_ns = ?,
                    hypothesis = ?, method = ?, outcome_status = ?,
                    hypothesis_met = ?, findings = ?, action_items = CAST(? AS JSON),
                    target_system = ?, target_environment = ?, blast_radius = ?,
                    recovery_time_s = ?, duration_s = ?, attestation = ?,
                    renewal_due_ns = ?, framework_refs = CAST(? AS VARCHAR[])
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
                    id,
                ],
            )?;
            let hash = rehash(&self.conn, id)?;
            self.conn.execute(
                "UPDATE manual_experiments SET content_hash = ? WHERE id = ?",
                params![hash, id],
            )?;
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
                        "framework_refs"
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
            let hash = rehash(&self.conn, id)?;
            self.conn.execute(
                "UPDATE manual_experiments SET content_hash = ? WHERE id = ?",
                params![hash, id],
            )?;
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

    /// Verify a submitted record. The reviewer must differ from the person
    /// who entered the record (segregation of duties).
    ///
    /// # Errors
    /// Returns `NotFound`, `WrongStatus` (not submitted), or `SelfReview`.
    pub fn verify_manual(
        &self,
        id: &str,
        reviewer: &str,
        note: Option<&str>,
    ) -> Result<(), ManualError> {
        self.review(id, reviewer, note, true)
    }

    /// Reject a submitted record (a review note is mandatory). The reviewer
    /// must differ from the person who entered the record.
    ///
    /// # Errors
    /// Returns `NotFound`, `WrongStatus`, `SelfReview`, or `Invalid` when the
    /// note is empty.
    pub fn reject_manual(&self, id: &str, reviewer: &str, note: &str) -> Result<(), ManualError> {
        if note.trim().is_empty() {
            return Err(ManualError::Invalid(
                "reject requires a non-empty review note".into(),
            ));
        }
        self.review(id, reviewer, Some(note), false)
    }

    fn review(
        &self,
        id: &str,
        reviewer: &str,
        note: Option<&str>,
        approve: bool,
    ) -> Result<(), ManualError> {
        let action = if approve { "verify" } else { "reject" };
        let to_status = if approve { "verified" } else { "rejected" };
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
            let entered_by = row
                .get("entered_by")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if reviewer.trim().is_empty() {
                return Err(ManualError::Invalid("reviewer must not be empty".into()));
            }
            if reviewer == entered_by {
                return Err(ManualError::SelfReview);
            }
            let prev_hash = row
                .get("content_hash")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            self.conn.execute(
                "UPDATE manual_experiments SET status = ?, reviewed_by = ?, \
                 reviewed_at_ns = ?, review_note = ? WHERE id = ?",
                params![to_status, reviewer, now_ns(), note, id],
            )?;
            let hash = rehash(&self.conn, id)?;
            self.conn.execute(
                "UPDATE manual_experiments SET content_hash = ? WHERE id = ?",
                params![hash, id],
            )?;
            append_audit(
                &self.conn,
                id,
                reviewer,
                action,
                &serde_json::json!({
                    "from_status": "submitted",
                    "to_status": to_status,
                    "note": note,
                }),
                Some(&prev_hash),
                &hash,
            )?;
            Ok(())
        })
    }

    /// Attach an external evidence link to a draft or submitted record.
    /// Returns the attachment id.
    ///
    /// # Errors
    /// Returns `NotFound`, `WrongStatus` (verified/rejected records are
    /// locked), or `Invalid` for an empty URI.
    pub fn add_manual_attachment(
        &self,
        experiment_id: &str,
        kind: AttachmentKind,
        uri: &str,
        label: Option<&str>,
        file_hash: Option<&str>,
        added_by: &str,
    ) -> Result<String, ManualError> {
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
