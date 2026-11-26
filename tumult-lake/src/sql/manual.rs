//! The statements of manual records, plans, sign-offs and attachments
//! (`manual/`).

pub(crate) const INSERT_SIGNOFF: &str = "INSERT INTO manual_signoffs (
       id, subject_id, subject_kind, role, signed_by, signed_at_ns, decision, note,
       signed_hash, prev_hash, link_hash
    ) VALUES (?,?,?,?,?,?,?,?,?,?,?)";

pub(crate) const RECORD_ROW: &str = "SELECT row_to_json(t) AS j FROM \
    (SELECT * FROM manual_experiments WHERE id = ?) AS t";

pub(crate) const PLAN_ROW: &str = "SELECT row_to_json(t) AS j FROM \
    (SELECT * FROM manual_exercise_plans WHERE id = ?) AS t";

pub(crate) const RESEAL_RECORD: &str =
    "UPDATE manual_experiments SET content_hash = ?, hash_version = ? WHERE id = ?";

pub(crate) const RESEAL_PLAN: &str =
    "UPDATE manual_exercise_plans SET content_hash = ? WHERE id = ?";

pub(crate) const INSERT_PLAN: &str = "INSERT INTO manual_exercise_plans (
       id, name, exercise_type, target_system, target_environment, function_id,
       scope, owner, planned_at_ns, due_at_ns, interval_days, rto_target_s,
       rpo_target_s, status, created_by, created_at_ns, content_hash
    ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)";

pub(crate) const CLOSE_PLAN: &str =
    "UPDATE manual_exercise_plans SET status = ?, updated_by = ?, updated_at_ns = ? \
    WHERE id = ?";

pub(crate) const LINKED_FUNCTIONS: &str = "SELECT linked_id FROM manual_experiment_links \
    WHERE experiment_id = ? AND kind = 'function' ORDER BY ordinal";

pub(crate) const UPDATE_PLAN: &str = "UPDATE manual_exercise_plans SET
       name = ?, exercise_type = ?, target_system = ?, target_environment = ?,
       function_id = ?, scope = ?, owner = ?, planned_at_ns = ?, due_at_ns = ?,
       interval_days = ?, rto_target_s = ?, rpo_target_s = ?,
       updated_by = ?, updated_at_ns = ?
    WHERE id = ?";

pub(crate) const PLAN: &str = "SELECT * FROM manual_exercise_plans WHERE id = ?";

pub(crate) const PLAN_AUDIT: &str =
    "SELECT * FROM manual_plan_audit WHERE plan_id = ? ORDER BY changed_at_ns ASC, id ASC";

pub(crate) const RECORDS_WITH_STATUS: &str =
    "SELECT * FROM manual_experiments WHERE status = ? ORDER BY entered_at_ns DESC";

pub(crate) const RECORDS: &str = "SELECT * FROM manual_experiments ORDER BY entered_at_ns DESC";

pub(crate) const RECORD: &str = "SELECT * FROM manual_experiments WHERE id = ?";

pub(crate) const RECORD_AUDIT: &str =
    "SELECT * FROM manual_experiment_audit WHERE experiment_id = ? \
    ORDER BY changed_at_ns ASC, id ASC";

pub(crate) const ATTACHMENTS: &str = "SELECT * FROM evidence_attachments WHERE experiment_id = ? \
    ORDER BY added_at_ns ASC";

pub(crate) const ATTACHMENTS_IN_ORDER: &str =
    "SELECT * FROM evidence_attachments WHERE experiment_id = ? ORDER BY added_at_ns, id";

pub(crate) const INSERT_STEP: &str = "INSERT INTO manual_exercise_steps (
       experiment_id, ordinal, title, actor, started_at_ns, ended_at_ns, result, note
    ) VALUES (?,?,?,?,?,?,?,?)";

pub(crate) const INSERT_ACTION: &str = "INSERT INTO manual_action_items (
       id, experiment_id, ordinal, title, owner, due_at_ns, ticket,
       severity, retest_kind, retest_id
    ) VALUES (?,?,?,?,?,?,?,?,?,?)";

pub(crate) const INSERT_LINK: &str = "INSERT INTO manual_experiment_links (
       experiment_id, ordinal, kind, linked_id, label
    ) VALUES (?,?,?,?,?)";

pub(crate) const INSERT_DRAFT: &str = "INSERT INTO manual_experiments (
       id, experiment_name, exercise_type, executed_at_ns, hypothesis, method,
       outcome_status, hypothesis_met, findings, action_items,
       target_system, target_environment, blast_radius,
       recovery_time_s, duration_s, origin,
       entered_by, entered_at_ns, attestation, status,
       renewal_due_ns, framework_refs, batch_id, content_hash,
       rto_target_s, rpo_target_s, rpo_measured_s, data_verified, participants,
       plan_id, findings_severity, tester, tester_kind,
       tlpt_attestation_ref, tlpt_attested_at_ns, tlpt_summary_ref, scenarios
    ) VALUES (?,?,?,?,?,?,?,?,?,CAST(? AS JSON),?,?,?,?,?,?,
              ?,?,?,?,?, CAST(? AS VARCHAR[]),?,?,?,?,?,?,CAST(? AS JSON),?,
              ?,?,?,?,?,?, CAST(? AS VARCHAR[]))";

pub(crate) const UPDATE_DRAFT: &str = "UPDATE manual_experiments SET
       experiment_name = ?, exercise_type = ?, executed_at_ns = ?,
       hypothesis = ?, method = ?, outcome_status = ?,
       hypothesis_met = ?, findings = ?, action_items = CAST(? AS JSON),
       target_system = ?, target_environment = ?, blast_radius = ?,
       recovery_time_s = ?, duration_s = ?, attestation = ?,
       renewal_due_ns = ?, framework_refs = CAST(? AS VARCHAR[]),
       rto_target_s = ?, rpo_target_s = ?, rpo_measured_s = ?,
       data_verified = ?, participants = CAST(? AS JSON), plan_id = ?,
       findings_severity = ?, tester = ?, tester_kind = ?,
       tlpt_attestation_ref = ?, tlpt_attested_at_ns = ?, tlpt_summary_ref = ?,
       scenarios = CAST(? AS VARCHAR[])
    WHERE id = ?";

pub(crate) const SUBMIT: &str =
    "UPDATE manual_experiments SET status = 'submitted', attestation = ? \
    WHERE id = ?";

pub(crate) const COUNT_APPROVALS: &str =
    "SELECT COUNT(*) FROM manual_signoffs WHERE subject_kind = 'record' \
    AND subject_id = ? AND decision = 'approved'";

pub(crate) const REVIEW: &str = "UPDATE manual_experiments SET status = ?, reviewed_by = ?, \
    reviewed_at_ns = ?, review_note = ? WHERE id = ?";

pub(crate) const COUNT_AUTHORED: &str = "SELECT COUNT(*) FROM manual_experiment_audit WHERE experiment_id = ? AND changed_by = ? AND action IN ('create', 'edit', 'submit', 'attach')";

pub(crate) const COUNT_SIGNED: &str =
    "SELECT COUNT(*) FROM manual_signoffs WHERE subject_kind = 'record' \
    AND subject_id = ? AND signed_by = ?";

pub(crate) const INSERT_ATTACHMENT: &str = "INSERT INTO evidence_attachments (
       id, experiment_id, kind, uri, label, file_hash, added_by, added_at_ns
    ) VALUES (?,?,?,?,?,?,?,?)";

pub(crate) const INSERT_FILE: &str = "INSERT INTO evidence_attachments (
       id, experiment_id, kind, uri, label, file_hash, added_by, added_at_ns,
       media_type, size_bytes
    ) VALUES (?,?,?,?,?,?,?,?,?,?)";

pub(crate) const INSERT_IMPORT_BATCH: &str =
    "INSERT INTO import_batches (id, source, imported_at_ns, rows, label) \
    VALUES (?,?,?,?,?)";

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        INSERT_SIGNOFF,
        RECORD_ROW,
        PLAN_ROW,
        RESEAL_RECORD,
        RESEAL_PLAN,
        INSERT_PLAN,
        CLOSE_PLAN,
        LINKED_FUNCTIONS,
        UPDATE_PLAN,
        PLAN,
        PLAN_AUDIT,
        RECORDS_WITH_STATUS,
        RECORDS,
        RECORD,
        RECORD_AUDIT,
        ATTACHMENTS,
        ATTACHMENTS_IN_ORDER,
        INSERT_STEP,
        INSERT_ACTION,
        INSERT_LINK,
        INSERT_DRAFT,
        UPDATE_DRAFT,
        SUBMIT,
        COUNT_APPROVALS,
        REVIEW,
        COUNT_AUTHORED,
        COUNT_SIGNED,
        INSERT_ATTACHMENT,
        INSERT_FILE,
        INSERT_IMPORT_BATCH
    ]
}
