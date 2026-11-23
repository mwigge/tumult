//! v2 manual-evidence DDL, in the v8 shape: `manual_experiments` carries
//! NO primary key and NO secondary indexes (same rule as the v5 run
//! tables): the lifecycle UPDATEs it receives would die fatally on
//! desynced ART indexes after a mid-write kill. The audit and attachment
//! tables are INSERT-only, so their indexes are safe to keep.
//!
//! v22 adds the DR record: the columns of [`V22_COLUMNS`] and the tables
//! below the v2 ones, index-free because a draft edit deletes and writes
//! the child rows again and plans are edited. The plan, plan audit and
//! sign-off tables and the attachment and audit columns are written by
//! later changes of the same version; they are created here so the lake
//! needs one migration for them.

/// Hand-entered test records (game days, tabletops, failovers, …) with
/// attestation, a draft → submitted → verified/rejected review lifecycle,
/// an append-only hash-chained audit trail, and external evidence links
/// (no file storage — URIs only).
///
/// `manual_exercise_steps`, `manual_action_items` and
/// `manual_experiment_links` hold the timeline, the actions with owners
/// and the links of a record, in `ordinal` order from 1. The action
/// columns from `status` on track the action after sign-off and are not
/// part of the sealed record. `manual_exercise_plans` holds the planned
/// exercises a record fulfils, `manual_plan_audit` their trail, and
/// `manual_signoffs` the sign-offs of a record or a plan.
pub(super) const DDL: &str = "
CREATE TABLE IF NOT EXISTS manual_experiments (
    id VARCHAR NOT NULL,
    experiment_name VARCHAR NOT NULL,
    exercise_type VARCHAR NOT NULL,
    executed_at_ns BIGINT NOT NULL,
    hypothesis VARCHAR NOT NULL,
    method VARCHAR NOT NULL,
    outcome_status VARCHAR NOT NULL,
    hypothesis_met BOOLEAN,
    findings VARCHAR,
    action_items JSON,
    target_system VARCHAR,
    target_environment VARCHAR,
    blast_radius VARCHAR,
    recovery_time_s DOUBLE,
    duration_s DOUBLE,
    origin VARCHAR NOT NULL DEFAULT 'manual',
    entered_by VARCHAR NOT NULL,
    entered_at_ns BIGINT NOT NULL,
    attestation VARCHAR NOT NULL,
    status VARCHAR NOT NULL DEFAULT 'draft',
    reviewed_by VARCHAR,
    reviewed_at_ns BIGINT,
    review_note VARCHAR,
    renewal_due_ns BIGINT,
    framework_refs VARCHAR[],
    batch_id VARCHAR,
    content_hash VARCHAR NOT NULL
);

CREATE TABLE IF NOT EXISTS manual_experiment_audit (
    id VARCHAR PRIMARY KEY,
    experiment_id VARCHAR NOT NULL,
    changed_by VARCHAR NOT NULL,
    changed_at_ns BIGINT NOT NULL,
    action VARCHAR NOT NULL,
    diff JSON,
    prev_hash VARCHAR,
    new_hash VARCHAR NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_manual_audit_experiment ON manual_experiment_audit (experiment_id);

CREATE TABLE IF NOT EXISTS evidence_attachments (
    id VARCHAR PRIMARY KEY,
    experiment_id VARCHAR NOT NULL,
    kind VARCHAR NOT NULL,
    uri VARCHAR NOT NULL,
    label VARCHAR,
    file_hash VARCHAR,
    added_by VARCHAR NOT NULL,
    added_at_ns BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_evidence_attachments_experiment ON evidence_attachments (
    experiment_id
);

CREATE TABLE IF NOT EXISTS manual_exercise_steps (
    experiment_id VARCHAR NOT NULL,
    ordinal INTEGER NOT NULL,
    title VARCHAR NOT NULL,
    actor VARCHAR,
    started_at_ns BIGINT,
    ended_at_ns BIGINT,
    result VARCHAR,
    note VARCHAR
);

CREATE TABLE IF NOT EXISTS manual_action_items (
    id VARCHAR NOT NULL,
    experiment_id VARCHAR NOT NULL,
    ordinal INTEGER NOT NULL,
    title VARCHAR NOT NULL,
    owner VARCHAR NOT NULL,
    due_at_ns BIGINT,
    ticket VARCHAR,
    status VARCHAR NOT NULL DEFAULT 'open',
    closed_at_ns BIGINT,
    closed_by VARCHAR
);

CREATE TABLE IF NOT EXISTS manual_experiment_links (
    experiment_id VARCHAR NOT NULL,
    ordinal INTEGER NOT NULL,
    kind VARCHAR NOT NULL,
    linked_id VARCHAR NOT NULL,
    label VARCHAR
);

CREATE TABLE IF NOT EXISTS manual_exercise_plans (
    id VARCHAR NOT NULL,
    name VARCHAR NOT NULL,
    exercise_type VARCHAR NOT NULL,
    target_system VARCHAR,
    target_environment VARCHAR,
    function_id VARCHAR,
    scope VARCHAR,
    owner VARCHAR NOT NULL,
    planned_at_ns BIGINT,
    due_at_ns BIGINT NOT NULL,
    interval_days INTEGER,
    rto_target_s DOUBLE,
    rpo_target_s DOUBLE,
    status VARCHAR NOT NULL DEFAULT 'planned',
    created_by VARCHAR NOT NULL,
    created_at_ns BIGINT NOT NULL,
    updated_by VARCHAR,
    updated_at_ns BIGINT,
    content_hash VARCHAR NOT NULL
);

CREATE TABLE IF NOT EXISTS manual_plan_audit (
    id VARCHAR NOT NULL,
    plan_id VARCHAR NOT NULL,
    changed_by VARCHAR NOT NULL,
    changed_at_ns BIGINT NOT NULL,
    action VARCHAR NOT NULL,
    diff JSON,
    prev_hash VARCHAR,
    new_hash VARCHAR NOT NULL
);

CREATE TABLE IF NOT EXISTS manual_signoffs (
    id VARCHAR NOT NULL,
    subject_id VARCHAR NOT NULL,
    subject_kind VARCHAR NOT NULL,
    role VARCHAR NOT NULL,
    signed_by VARCHAR NOT NULL,
    signed_at_ns BIGINT NOT NULL,
    decision VARCHAR NOT NULL,
    note VARCHAR,
    signed_hash VARCHAR NOT NULL,
    prev_hash VARCHAR,
    link_hash VARCHAR NOT NULL
);
";

/// The v22 columns of the v2 tables, NULL on the rows stored before. On
/// `manual_experiments` they run after the v8 rebuild (see
/// `store::migrate`): it copies the v8 columns by position, so a column
/// added before it would break the copy of a v2 to v7 lake.
///
/// `rto_target_s` and `rpo_target_s` are the planned RTO and RPO,
/// `rpo_measured_s` the actual RPO (the actual RTO is `recovery_time_s`).
/// An RPO is the seconds of committed writes from before the outage that
/// are lost, as the automated RPO metric means it. `data_verified` says
/// whether the data was checked after the recovery and found complete
/// (NULL: not checked). `participants` is a JSON list of `name` and
/// `role`. `plan_id` names the plan a record fulfils. `hash_version` is
/// the form `content_hash` was computed in: 1 for a row sealed before v22,
/// which keeps it until its next lifecycle write. On the attachments,
/// `media_type` and `size_bytes` describe a stored file; on the record
/// and plan audit, `link_hash` chains the audit rows themselves.
/// `manual_plan_audit.link_hash` came after the table: a v22 lake written
/// before it has the table without the column, which `CREATE TABLE IF NOT
/// EXISTS` does not add, so it is added here like the others.
pub(crate) const V22_COLUMNS: &str = "
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS rto_target_s DOUBLE;
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS rpo_target_s DOUBLE;
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS rpo_measured_s DOUBLE;
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS data_verified BOOLEAN;
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS participants JSON;
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS plan_id VARCHAR;
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS hash_version INTEGER DEFAULT 1;
ALTER TABLE evidence_attachments ADD COLUMN IF NOT EXISTS media_type VARCHAR;
ALTER TABLE evidence_attachments ADD COLUMN IF NOT EXISTS size_bytes BIGINT;
ALTER TABLE manual_experiment_audit ADD COLUMN IF NOT EXISTS link_hash VARCHAR;
ALTER TABLE manual_plan_audit ADD COLUMN IF NOT EXISTS link_hash VARCHAR;
";
