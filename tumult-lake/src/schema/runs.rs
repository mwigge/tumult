//! v4–v7 daemon-run DDL. `run_registry` holds validated .toon definitions
//! (content-hash deduped); `runs` is the run state machine
//! (queued→validating→running→stopping→passed|deviated|failed|aborted, plus
//! orphaned/rollback_pending for crash recovery; T10's pending_approval is
//! a value-level addition, no schema change); `run_audit` is the append-only
//! per-run event trail.
//!
//! v5: these tables carry NO primary keys and NO secondary indexes. A daemon
//! killed mid-write (SIGKILL) can come back with DuckDB's ART indexes
//! desynced from the table after WAL replay, and every subsequent UPDATE
//! then dies fatally on 'Failed to delete all rows from index' — poisoning
//! the store exactly when orphan reconciliation must write. At run-table
//! scale (thousands of rows) sequential scans are free; run-id uniqueness
//! comes from uuid generation and registry dedup is checked in code.

/// Run tables (v5 shape), the v6 `run_audit.actor` column, the v7 approval
/// tables and the v7 audit hash chain.
pub(super) const DDL: &str = "
CREATE TABLE IF NOT EXISTS run_registry (
    id               VARCHAR NOT NULL,
    name             VARCHAR NOT NULL,
    definition_toon  VARCHAR NOT NULL,
    content_hash     VARCHAR NOT NULL,
    registered_at_ns BIGINT NOT NULL,
    registered_by    VARCHAR
);
CREATE TABLE IF NOT EXISTS runs (
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
CREATE TABLE IF NOT EXISTS run_audit (
    run_id  VARCHAR NOT NULL,
    at_ns   BIGINT NOT NULL,
    event   VARCHAR NOT NULL,
    detail  VARCHAR
);
-- v6: `actor` is the authenticated session identity on run audit events
-- (NULL for system events). Additive and idempotent, like the v1.1
-- metric_histograms ALTERs above.
ALTER TABLE run_audit ADD COLUMN IF NOT EXISTS actor VARCHAR;

-- v7: approval workflows (T10, ADR-013). `approval_requests` pins one run
-- to exactly one canonical content hash (definition + params + env +
-- target) with a TTL and a quorum; `approval_decisions` holds one row per
-- approver decision (T3's quorum 2 = two approved rows from distinct
-- approvers). T0 runs never gate and appear in neither table. Same
-- index-free rule as the v5 run tables: sequential scans are free at this
-- scale; one-decision-per-approver and approver≠requester are enforced in
-- code (`Writer::insert_approval_decision`).
CREATE TABLE IF NOT EXISTS approval_requests (
    run_id VARCHAR NOT NULL,
    tier VARCHAR NOT NULL,              -- T1|T2|T3
    pin_hash VARCHAR NOT NULL,          -- sha256 hex of the canonical pin
    env VARCHAR NOT NULL,
    target VARCHAR,
    quorum_required INTEGER NOT NULL,
    requested_by VARCHAR NOT NULL,
    requested_at_ns BIGINT NOT NULL,
    expires_at_ns BIGINT NOT NULL,
    consumed_at_ns BIGINT,              -- single-use: stamped at dispatch
    break_glass BOOLEAN NOT NULL DEFAULT FALSE,
    break_glass_by VARCHAR,
    break_glass_justification VARCHAR
);
CREATE TABLE IF NOT EXISTS approval_decisions (
    run_id VARCHAR NOT NULL,
    approver VARCHAR NOT NULL,
    decision VARCHAR NOT NULL,          -- approved|rejected
    note VARCHAR,
    decided_at_ns BIGINT NOT NULL
);
-- v7: the run audit hash chain. Every new event's `new_hash` covers the
-- event content plus the previous link's hash (NULL for a run's first
-- link); pre-v7 rows keep NULL hashes and are treated as legacy by
-- `Reader::verify_run_audit_chain`.
ALTER TABLE run_audit ADD COLUMN IF NOT EXISTS prev_hash VARCHAR;
ALTER TABLE run_audit ADD COLUMN IF NOT EXISTS new_hash VARCHAR;
";
