//! Schema v3 DDL.
//!
//! Wide, ClickHouse-exporter-aligned tables plus `MAP(VARCHAR, VARCHAR)`
//! attribute maps for the dynamic tail (e.g. `resilience.baseline.probe.{name}.*`).
//! Low-cardinality, high-selectivity `resilience.*` keys are materialized as
//! columns by the ingest layer; everything else stays in the maps.
//!
//! v2 adds the manual-evidence tables (`manual_experiments`,
//! `manual_experiment_audit`, `evidence_attachments`) — see `manual.rs`.
//!
//! v3 unifies the tumult-analytics schema family into the same database
//! under unchanged table names: `experiments` / `activity_results` /
//! `load_results` (journal detail), the four `agentic_*` tables, the three
//! `autopilot_*` tables, and the ChaosGraph `graph_nodes` / `graph_edges`
//! tables (DDL owned by `tumult_graph::sql`, executed at migrate time).
//! One database file, one writer, one `schema_meta`.
//!
//! v4 adds the daemon-run tables (`run_registry`, `runs`, `run_audit`) —
//! see `runs.rs`.
//!
//! v5 rebuilds the v4 run tables without primary keys or secondary indexes:
//! a daemon killed mid-write can return with DuckDB's ART indexes desynced
//! from the table after WAL replay, and every UPDATE then fails fatally
//! ("Failed to delete all rows from index"), poisoning the store exactly
//! when orphan reconciliation must write. Run tables are tiny; scans are
//! free and uniqueness is enforced in code.

pub const CURRENT_SCHEMA_VERSION: i64 = 5;

/// All DDL is `IF NOT EXISTS`, so this doubles as the idempotent v0 → v1
/// migration on every open.
pub const CREATE_TABLES: &str = "
CREATE TABLE IF NOT EXISTS spans (
    ts_ns BIGINT NOT NULL,
    trace_id VARCHAR NOT NULL,
    span_id VARCHAR NOT NULL,
    parent_span_id VARCHAR,
    span_name VARCHAR NOT NULL,
    span_kind VARCHAR NOT NULL,
    duration_ns BIGINT NOT NULL,
    status_code VARCHAR NOT NULL,
    status_message VARCHAR NOT NULL,
    service_name VARCHAR NOT NULL,
    service_version VARCHAR,
    experiment_id VARCHAR,
    experiment_name VARCHAR,
    outcome_status VARCHAR,
    fault_type VARCHAR,
    fault_subtype VARCHAR,
    fault_severity VARCHAR,
    blast_radius VARCHAR,
    target_system VARCHAR,
    target_technology VARCHAR,
    target_environment VARCHAR,
    plugin_name VARCHAR,
    hypothesis_met BOOLEAN,
    recovery_time_s DOUBLE,
    span_attrs MAP(VARCHAR, VARCHAR),
    resource_attrs MAP(VARCHAR, VARCHAR),
    events JSON
);
CREATE INDEX IF NOT EXISTS idx_spans_ts_ns ON spans (ts_ns);
CREATE INDEX IF NOT EXISTS idx_spans_experiment_id ON spans (experiment_id);
CREATE INDEX IF NOT EXISTS idx_spans_trace_id ON spans (trace_id);

CREATE TABLE IF NOT EXISTS logs (
    ts_ns BIGINT NOT NULL,
    severity_text VARCHAR NOT NULL,
    body VARCHAR NOT NULL,
    trace_id VARCHAR,
    span_id VARCHAR,
    service_name VARCHAR NOT NULL,
    log_attrs MAP(VARCHAR, VARCHAR),
    resource_attrs MAP(VARCHAR, VARCHAR)
);
CREATE INDEX IF NOT EXISTS idx_logs_ts_ns ON logs (ts_ns);

CREATE TABLE IF NOT EXISTS metric_sums (
    ts_ns BIGINT NOT NULL,
    metric_name VARCHAR NOT NULL,
    value DOUBLE NOT NULL,
    experiment_name VARCHAR,
    outcome_status VARCHAR,
    plugin_name VARCHAR,
    attrs MAP(VARCHAR, VARCHAR),
    resource_attrs MAP(VARCHAR, VARCHAR)
);
CREATE INDEX IF NOT EXISTS idx_metric_sums_name_ts ON metric_sums (metric_name, ts_ns);

CREATE TABLE IF NOT EXISTS metric_gauges (
    ts_ns BIGINT NOT NULL,
    metric_name VARCHAR NOT NULL,
    value DOUBLE NOT NULL,
    experiment_name VARCHAR,
    outcome_status VARCHAR,
    plugin_name VARCHAR,
    attrs MAP(VARCHAR, VARCHAR),
    resource_attrs MAP(VARCHAR, VARCHAR)
);
CREATE INDEX IF NOT EXISTS idx_metric_gauges_name_ts ON metric_gauges (metric_name, ts_ns);

CREATE TABLE IF NOT EXISTS metric_histograms (
    ts_ns BIGINT NOT NULL,
    metric_name VARCHAR NOT NULL,
    count UBIGINT NOT NULL,
    sum DOUBLE NOT NULL,
    min DOUBLE,
    max DOUBLE,
    bucket_counts BIGINT[],
    explicit_bounds DOUBLE[],
    attrs MAP(VARCHAR, VARCHAR),
    resource_attrs MAP(VARCHAR, VARCHAR),
    experiment_name VARCHAR,
    outcome_status VARCHAR,
    plugin_name VARCHAR
);
-- v1 → v1.1: promoted dim columns for pre-existing databases (the CREATE
-- above already includes them; ADD COLUMN IF NOT EXISTS makes this a no-op
-- there).
ALTER TABLE metric_histograms ADD COLUMN IF NOT EXISTS experiment_name VARCHAR;
ALTER TABLE metric_histograms ADD COLUMN IF NOT EXISTS outcome_status VARCHAR;
ALTER TABLE metric_histograms ADD COLUMN IF NOT EXISTS plugin_name VARCHAR;
CREATE INDEX IF NOT EXISTS idx_metric_histograms_name_ts
    ON metric_histograms (metric_name, ts_ns);

CREATE TABLE IF NOT EXISTS import_batches (
    id VARCHAR NOT NULL,
    source VARCHAR NOT NULL,
    imported_at_ns BIGINT NOT NULL,
    rows INTEGER NOT NULL,
    label VARCHAR
);

CREATE TABLE IF NOT EXISTS schema_meta (
    key VARCHAR PRIMARY KEY,
    value BIGINT NOT NULL
);

-- v2: manual evidence. Hand-entered test records (game days, tabletops,
-- failovers, …) with attestation, a draft → submitted → verified/rejected
-- review lifecycle, an append-only hash-chained audit trail, and external
-- evidence links (no file storage — URIs only).
CREATE TABLE IF NOT EXISTS manual_experiments (
    id VARCHAR PRIMARY KEY,
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
CREATE INDEX IF NOT EXISTS idx_manual_experiments_name ON manual_experiments (experiment_name);
CREATE INDEX IF NOT EXISTS idx_manual_experiments_status ON manual_experiments (status);

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

-- v3: the tumult-analytics family, unified into this database under
-- unchanged table names so TUI/MCP/CLI SQL keeps working. `experiments`
-- stays the authoritative journal-detail table; `experiment_runs` (the
-- span-rollup view above) joins to it by experiment id.
CREATE TABLE IF NOT EXISTS experiments (
    experiment_id VARCHAR NOT NULL, title VARCHAR NOT NULL,
    status VARCHAR NOT NULL, started_at_ns BIGINT NOT NULL,
    ended_at_ns BIGINT NOT NULL, duration_ms UBIGINT NOT NULL,
    method_step_count BIGINT NOT NULL, rollback_count BIGINT NOT NULL,
    hypothesis_before_met BOOLEAN, hypothesis_after_met BOOLEAN,
    estimate_accuracy DOUBLE, resilience_score DOUBLE
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_experiments_id
    ON experiments (experiment_id);
CREATE TABLE IF NOT EXISTS activity_results (
    experiment_id VARCHAR NOT NULL, name VARCHAR NOT NULL,
    activity_type VARCHAR NOT NULL, status VARCHAR NOT NULL,
    started_at_ns BIGINT NOT NULL, duration_ms UBIGINT NOT NULL,
    output VARCHAR, error VARCHAR, phase VARCHAR NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_activities_experiment_id
    ON activity_results (experiment_id);
CREATE TABLE IF NOT EXISTS load_results (
    experiment_id VARCHAR NOT NULL, tool VARCHAR NOT NULL,
    started_at_ns BIGINT NOT NULL, ended_at_ns BIGINT NOT NULL,
    duration_s DOUBLE NOT NULL, vus INTEGER NOT NULL,
    throughput_rps DOUBLE NOT NULL, latency_p50_ms DOUBLE NOT NULL,
    latency_p95_ms DOUBLE NOT NULL, latency_p99_ms DOUBLE NOT NULL,
    error_rate DOUBLE NOT NULL, total_requests UBIGINT NOT NULL,
    thresholds_met BOOLEAN NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_load_experiment_id
    ON load_results (experiment_id);
CREATE TABLE IF NOT EXISTS agentic_runs (
    run_id VARCHAR PRIMARY KEY, experiment_id VARCHAR NOT NULL,
    target_type VARCHAR NOT NULL, scenario VARCHAR NOT NULL,
    resilience_score DOUBLE NOT NULL, trace_id VARCHAR, replay_id VARCHAR
);
CREATE INDEX IF NOT EXISTS idx_agentic_runs_experiment_id
    ON agentic_runs (experiment_id);
CREATE TABLE IF NOT EXISTS agentic_contract_outcomes (
    run_id VARCHAR NOT NULL, scenario VARCHAR NOT NULL,
    contract_type VARCHAR NOT NULL, passed BOOLEAN NOT NULL,
    reason VARCHAR, severity DOUBLE NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agentic_contracts_run_id
    ON agentic_contract_outcomes (run_id);
CREATE TABLE IF NOT EXISTS agentic_fault_applications (
    run_id VARCHAR NOT NULL, scenario VARCHAR NOT NULL,
    fault_type VARCHAR NOT NULL, applied BOOLEAN NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agentic_faults_run_id
    ON agentic_fault_applications (run_id);
CREATE TABLE IF NOT EXISTS agentic_replay_outcomes (
    run_id VARCHAR NOT NULL, replay_id VARCHAR NOT NULL,
    scenario VARCHAR NOT NULL, passed BOOLEAN NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agentic_replay_run_id
    ON agentic_replay_outcomes (run_id);

-- v3: autopilot decision store (INSERT-ONLY, event-sourcing style).
CREATE TABLE IF NOT EXISTS autopilot_decisions (
    id              VARCHAR PRIMARY KEY,
    decided_at_ns   BIGINT NOT NULL,
    trigger         VARCHAR NOT NULL,
    service_id      VARCHAR NOT NULL,
    tier            VARCHAR,
    plugin          VARCHAR NOT NULL,
    action          VARCHAR NOT NULL,
    article_id      VARCHAR NOT NULL,
    score           DOUBLE NOT NULL,
    reasons         JSON NOT NULL,
    confidence      VARCHAR NOT NULL,
    playbook        VARCHAR,
    validator       JSON NOT NULL,
    verdict         VARCHAR NOT NULL,
    gate_rules      JSON NOT NULL,
    gate_detail     JSON NOT NULL,
    policy_hash     VARCHAR NOT NULL,
    autonomy_score  DOUBLE
);
CREATE TABLE IF NOT EXISTS autopilot_events (
    decision_id     VARCHAR NOT NULL,
    at_ns           BIGINT NOT NULL,
    kind            VARCHAR NOT NULL,
    detail          JSON NOT NULL
);
CREATE INDEX IF NOT EXISTS autopilot_events_by_decision
    ON autopilot_events (decision_id, at_ns);
CREATE TABLE IF NOT EXISTS autopilot_change_events (
    service_id      VARCHAR NOT NULL,
    at_ns           BIGINT NOT NULL,
    source          VARCHAR NOT NULL,
    detail          VARCHAR
);

-- v4: daemon-run tables. `run_registry` holds validated .toon definitions
-- (content-hash deduped); `runs` is the run state machine
-- (queued→validating→running→stopping→passed|deviated|failed|aborted, plus
-- orphaned/rollback_pending for crash recovery; T10's pending_approval is
-- a value-level addition, no schema change); `run_audit` is the append-only
-- per-run event trail.
-- v5: these tables carry NO primary keys and NO secondary indexes. A daemon
-- killed mid-write (SIGKILL) can come back with DuckDB's ART indexes
-- desynced from the table after WAL replay, and every subsequent UPDATE
-- then dies fatally on 'Failed to delete all rows from index' — poisoning
-- the store exactly when orphan reconciliation must write. At run-table
-- scale (thousands of rows) sequential scans are free; run-id uniqueness
-- comes from uuid generation and registry dedup is checked in code.
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
";

/// v4 → v5: rebuild the run tables without primary keys / secondary indexes
/// (see the comment above). Data copy is a plain table scan — safe even when
/// the v4 ART indexes are desynced, since reads never touch them. Atomic:
/// any failure rolls back and the next open retries (version stays 4).
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
INSERT INTO runs_v5 SELECT * FROM runs;
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
INSERT INTO run_registry_v5 SELECT * FROM run_registry;
DROP TABLE run_registry;
ALTER TABLE run_registry_v5 RENAME TO run_registry;
DROP INDEX IF EXISTS idx_run_audit_run;
COMMIT;
";

/// Rollup view: one row per experiment run, over the experiment root spans
/// tumult emits as `resilience.experiment`. The outcome lives on tumult's
/// `experiment.completed` log record (capitalised `status` attr), not on the
/// span, so it is resolved via the same join the API list query uses.
/// CREATE OR REPLACE so existing databases pick up view changes on startup.
pub const CREATE_VIEWS: &str = "
CREATE OR REPLACE VIEW experiment_runs AS
SELECT
    s.experiment_id,
    any_value(s.experiment_name) AS experiment_name,
    min(s.ts_ns) AS started_at_ns,
    max(s.ts_ns + s.duration_ns) AS ended_at_ns,
    max(s.duration_ns) AS duration_ns,
    any_value(coalesce(s.outcome_status, l.log_attrs['status'])) AS outcome_status,
    any_value(s.hypothesis_met) AS hypothesis_met
FROM spans s
LEFT JOIN logs l
    ON l.log_attrs['experiment_id'] = s.experiment_id
    AND l.body = 'experiment.completed'
WHERE s.span_name = 'resilience.experiment'
GROUP BY s.experiment_id;
";
