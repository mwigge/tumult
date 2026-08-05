//! v2 manual-evidence DDL, in the v8 shape: `manual_experiments` carries
//! NO primary key and NO secondary indexes (same rule as the v5 run
//! tables): the lifecycle UPDATEs it receives would die fatally on
//! desynced ART indexes after a mid-write kill. The audit and attachment
//! tables are INSERT-only, so their indexes are safe to keep.

/// Hand-entered test records (game days, tabletops, failovers, …) with
/// attestation, a draft → submitted → verified/rejected review lifecycle,
/// an append-only hash-chained audit trail, and external evidence links
/// (no file storage — URIs only).
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
CREATE INDEX IF NOT EXISTS idx_evidence_attachments_experiment
    ON evidence_attachments (experiment_id);
";
