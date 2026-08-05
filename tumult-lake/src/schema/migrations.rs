//! Versioned schema rebuilds: table-replacement migrations executed only
//! when the stored schema version requires them (see `store::migrate`).

/// v4 → v5: rebuild the run tables without primary keys / secondary indexes
/// (see the comment above). Data copy is a plain table scan — safe even when
/// the v4 ART indexes are desynced, since reads never touch them. Atomic:
/// any failure rolls back and the next open retries (version stays 4).
/// Explicit column lists, not SELECT *: later additive migrations (v12's
/// gameday columns) ALTER the old table before this rebuild runs, and a
/// wildcard then pulls more columns than the v5 table has.
pub const MIGRATE_V5_RUN_TABLES_INDEX_FREE: &str = "
BEGIN TRANSACTION;
CREATE TABLE runs_v5 (
    id              VARCHAR NOT NULL,
    registry_id     VARCHAR NOT NULL,
    state           VARCHAR NOT NULL,
    params_json     JSON,
    experiment_id   VARCHAR,
    rollback_status VARCHAR,
    error           VARCHAR,
    queued_at_ns    BIGINT NOT NULL,
    started_at_ns   BIGINT,
    ended_at_ns     BIGINT
);
INSERT INTO runs_v5 SELECT id, registry_id, state, params_json, experiment_id,
    rollback_status, error, queued_at_ns, started_at_ns, ended_at_ns FROM runs;
DROP TABLE runs;
ALTER TABLE runs_v5 RENAME TO runs;
CREATE TABLE run_registry_v5 (
    id               VARCHAR NOT NULL,
    name             VARCHAR NOT NULL,
    definition_toon  VARCHAR NOT NULL,
    content_hash     VARCHAR NOT NULL,
    registered_at_ns BIGINT NOT NULL,
    registered_by    VARCHAR
);
INSERT INTO run_registry_v5 SELECT id, name, definition_toon, content_hash,
    registered_at_ns, registered_by FROM run_registry;
DROP TABLE run_registry;
ALTER TABLE run_registry_v5 RENAME TO run_registry;
DROP INDEX IF EXISTS idx_run_audit_run;
COMMIT;
";

/// v2–v7 → v8: rebuild `manual_experiments` without the primary key /
/// secondary indexes (see the comment above). Data copy is a plain table
/// scan — safe even when the ART indexes are desynced, since reads never
/// touch them. Atomic: any failure rolls back and the next open retries.
/// The INSERT-only audit / attachment tables are untouched by design.
pub const MIGRATE_V8_MANUAL_EXPERIMENTS_INDEX_FREE: &str = "
BEGIN TRANSACTION;
CREATE TABLE manual_experiments_v8 (
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
INSERT INTO manual_experiments_v8 SELECT * FROM manual_experiments;
DROP TABLE manual_experiments;
ALTER TABLE manual_experiments_v8 RENAME TO manual_experiments;
COMMIT;
";
