-- The SQL corpus of tumult-lake: every statement the crate runs, as its
-- code renders it, each under the file and the item that hold it. A test
-- of the crate prepares each statement on a fresh lake and keeps this
-- file; CI lints it with sqlfluff. Change the SQL in the code, then write
-- this file again and read the difference:
--   TUMULT_BLESS=1 cargo test -p tumult-lake -- sql_corpus

-- tumult-lake/src/schema/mod.rs:telemetry::DDL
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
    span_attrs MAP (VARCHAR, VARCHAR),
    resource_attrs MAP (VARCHAR, VARCHAR),
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
    log_attrs MAP (VARCHAR, VARCHAR),
    resource_attrs MAP (VARCHAR, VARCHAR),
    event_name VARCHAR,
    scope_attrs MAP (VARCHAR, VARCHAR)
);
-- v20 → v21: the OTLP EventName and the scope attributes of a record, for
-- pre-existing databases (a no-op on a table the CREATE above built).
ALTER TABLE logs ADD COLUMN IF NOT EXISTS event_name VARCHAR;
ALTER TABLE logs ADD COLUMN IF NOT EXISTS scope_attrs MAP (VARCHAR, VARCHAR);
CREATE INDEX IF NOT EXISTS idx_logs_ts_ns ON logs (ts_ns);

CREATE TABLE IF NOT EXISTS metric_sums (
    ts_ns BIGINT NOT NULL,
    metric_name VARCHAR NOT NULL,
    value DOUBLE NOT NULL,
    experiment_name VARCHAR,
    outcome_status VARCHAR,
    plugin_name VARCHAR,
    attrs MAP (VARCHAR, VARCHAR),
    resource_attrs MAP (VARCHAR, VARCHAR)
);
CREATE INDEX IF NOT EXISTS idx_metric_sums_name_ts ON metric_sums (metric_name, ts_ns);

CREATE TABLE IF NOT EXISTS metric_gauges (
    ts_ns BIGINT NOT NULL,
    metric_name VARCHAR NOT NULL,
    value DOUBLE NOT NULL,
    experiment_name VARCHAR,
    outcome_status VARCHAR,
    plugin_name VARCHAR,
    attrs MAP (VARCHAR, VARCHAR),
    resource_attrs MAP (VARCHAR, VARCHAR)
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
    attrs MAP (VARCHAR, VARCHAR),
    resource_attrs MAP (VARCHAR, VARCHAR),
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

-- tumult-lake/src/schema/mod.rs:manual::DDL
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

-- tumult-lake/src/schema/mod.rs:analytics::DDL
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

CREATE TABLE IF NOT EXISTS autopilot_decisions (
    id VARCHAR PRIMARY KEY,
    decided_at_ns BIGINT NOT NULL,
    trigger VARCHAR NOT NULL,
    service_id VARCHAR NOT NULL,
    tier VARCHAR,
    plugin VARCHAR NOT NULL,
    action VARCHAR NOT NULL,
    article_id VARCHAR NOT NULL,
    score DOUBLE NOT NULL,
    reasons JSON NOT NULL,
    confidence VARCHAR NOT NULL,
    playbook VARCHAR,
    validator JSON NOT NULL,
    verdict VARCHAR NOT NULL,
    gate_rules JSON NOT NULL,
    gate_detail JSON NOT NULL,
    policy_hash VARCHAR NOT NULL,
    autonomy_score DOUBLE
);
CREATE TABLE IF NOT EXISTS autopilot_events (
    decision_id VARCHAR NOT NULL,
    at_ns BIGINT NOT NULL,
    kind VARCHAR NOT NULL,
    detail JSON NOT NULL
);
CREATE INDEX IF NOT EXISTS autopilot_events_by_decision
ON autopilot_events (decision_id, at_ns);
CREATE TABLE IF NOT EXISTS autopilot_change_events (
    service_id VARCHAR NOT NULL,
    at_ns BIGINT NOT NULL,
    source VARCHAR NOT NULL,
    detail VARCHAR
);

-- tumult-lake/src/schema/mod.rs:runs::DDL
CREATE TABLE IF NOT EXISTS run_registry (
    id VARCHAR NOT NULL,
    name VARCHAR NOT NULL,
    definition_toon VARCHAR NOT NULL,
    content_hash VARCHAR NOT NULL,
    registered_at_ns BIGINT NOT NULL,
    registered_by VARCHAR
);
CREATE TABLE IF NOT EXISTS runs (
    id VARCHAR NOT NULL,
    registry_id VARCHAR NOT NULL,
    state VARCHAR NOT NULL,
    params_json JSON,
    experiment_id VARCHAR,
    rollback_status VARCHAR,
    error VARCHAR,
    queued_at_ns BIGINT NOT NULL,
    started_at_ns BIGINT,
    ended_at_ns BIGINT
);
CREATE TABLE IF NOT EXISTS run_audit (
    run_id VARCHAR NOT NULL,
    at_ns BIGINT NOT NULL,
    event VARCHAR NOT NULL,
    detail VARCHAR
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

-- v14: immutable cleanup target bindings; secret references only.
CREATE TABLE IF NOT EXISTS run_recovery_plans (
    run_id VARCHAR NOT NULL,
    plan_json JSON NOT NULL
);
-- v15: resolved execution fingerprints are credential verifiers, not public audit.
CREATE TABLE IF NOT EXISTS run_execution_pins (
    run_id VARCHAR NOT NULL,
    execution_hash VARCHAR NOT NULL
);

-- tumult-lake/src/schema/mod.rs:auth::DDL
CREATE TABLE IF NOT EXISTS users (
    id VARCHAR NOT NULL,   -- uuid; the bootstrap 'legacy' user uses id 'legacy'
    username VARCHAR NOT NULL,   -- unique at code level
    password_hash VARCHAR NOT NULL,   -- argon2id PHC string; '!' = can never verify (legacy)
    role VARCHAR NOT NULL,   -- viewer|operator|approver|admin
    must_change BOOLEAN NOT NULL,   -- one-time bootstrap password must be changed
    disabled BOOLEAN NOT NULL,
    created_at_ns BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS sessions (
    id_hash VARCHAR NOT NULL,   -- sha256 hex of the opaque session id (cookie value never stored)
    user_id VARCHAR NOT NULL,
    created_at_ns BIGINT NOT NULL,
    expires_at_ns BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS tokens (
    id VARCHAR NOT NULL,   -- uuid of the token record (revocation handle)
    user_id VARCHAR NOT NULL,
    name VARCHAR NOT NULL,   -- human label, e.g. 'deploy script'
    token_hash VARCHAR NOT NULL,   -- sha256 hex of the kro_-prefixed token
    created_at_ns BIGINT NOT NULL,
    last_used_at_ns BIGINT,
    revoked BOOLEAN NOT NULL
);
-- v9: optional token expiry (NULL = never expires; pre-v9 tokens keep
-- working). Additive and idempotent, like the v6/v7 ALTERs above.
ALTER TABLE tokens ADD COLUMN IF NOT EXISTS expires_at_ns BIGINT;
CREATE TABLE IF NOT EXISTS user_env_scopes (
    user_id VARCHAR NOT NULL,
    environment VARCHAR NOT NULL    -- one row per allowed env; zero rows = all environments
);

-- tumult-lake/src/schema/mod.rs:table_stakes::DDL
CREATE TABLE IF NOT EXISTS run_schedules (
    id VARCHAR NOT NULL,   -- uuid
    name VARCHAR NOT NULL,
    registry_id VARCHAR NOT NULL,   -- references run_registry.id
    interval_s BIGINT NOT NULL,    -- seconds between fires
    vars_json VARCHAR,            -- template vars, same shape as runs.params_json
    env VARCHAR NOT NULL,   -- tier-classified at fire time (default 'dev')
    target VARCHAR,
    enabled BOOLEAN NOT NULL,
    next_run_at_ns BIGINT NOT NULL,
    last_run_at_ns BIGINT,
    last_run_id VARCHAR,
    created_by VARCHAR,
    created_at_ns BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS webhooks (
    id VARCHAR NOT NULL,   -- uuid
    name VARCHAR NOT NULL,
    url VARCHAR NOT NULL,
    secret VARCHAR NOT NULL,   -- HMAC key; never serialized by the API
    events VARCHAR NOT NULL,   -- JSON array of event names; [] = all
    enabled BOOLEAN NOT NULL,
    created_by VARCHAR,
    created_at_ns BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS webhook_cursors (
    webhook_id VARCHAR NOT NULL,
    last_at_ns BIGINT NOT NULL     -- run_audit position delivered up to
);

-- v13: webhook dead letters. Same index-free rule. One row per audit event
-- the dispatcher gave up on after bounded retries; `run_audit` remains the
-- source of truth for replay, this table is the record of the loss.
CREATE TABLE IF NOT EXISTS webhook_dead_letters (
    webhook_id VARCHAR NOT NULL,
    run_id VARCHAR NOT NULL,
    at_ns BIGINT NOT NULL,    -- the audit event's original timestamp
    event VARCHAR NOT NULL,
    detail VARCHAR,
    actor VARCHAR,
    error VARCHAR,            -- last delivery error
    attempts INTEGER NOT NULL,   -- consecutive failed dispatch ticks
    dead_at_ns BIGINT NOT NULL     -- when the dispatcher gave up
);

-- v12: GameDay campaigns (additive, like the v9 token-expiry ALTER).
ALTER TABLE run_registry ADD COLUMN IF NOT EXISTS kind VARCHAR;
ALTER TABLE runs ADD COLUMN IF NOT EXISTS gameday_id VARCHAR;

-- tumult-lake/src/schema/mod.rs:objectives::DDL
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS test_kind VARCHAR;
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS rto_target_s DOUBLE;
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS rpo_target_s DOUBLE;
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS rto_measured_s DOUBLE;
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS rpo_measured_s DOUBLE;
CREATE TABLE IF NOT EXISTS probe_samples (
    experiment_id VARCHAR NOT NULL, probe_name VARCHAR NOT NULL,
    phase VARCHAR NOT NULL, ts_ns BIGINT NOT NULL,
    value DOUBLE NOT NULL, ok BOOLEAN NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_probe_samples_experiment_id ON probe_samples (experiment_id);
CREATE TABLE IF NOT EXISTS nfr_verdicts (
    experiment_id VARCHAR NOT NULL, nfr_id VARCHAR NOT NULL,
    kind VARCHAR NOT NULL, metric VARCHAR NOT NULL,
    operator VARCHAR NOT NULL, threshold DOUBLE NOT NULL,
    observed DOUBLE, state VARCHAR NOT NULL, margin DOUBLE,
    criticality VARCHAR NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_nfr_verdicts_experiment_id ON nfr_verdicts (experiment_id);

-- tumult-lake/src/schema/mod.rs:objectives::READING_COLUMN
ALTER TABLE probe_samples ADD COLUMN IF NOT EXISTS reading DOUBLE;

-- tumult-lake/src/schema/mod.rs:risk::DDL
CREATE TABLE IF NOT EXISTS risk_suppressions (
    id VARCHAR NOT NULL, risk_id VARCHAR NOT NULL,
    reason VARCHAR NOT NULL, actor VARCHAR NOT NULL,
    at_ns BIGINT NOT NULL, expires_at_ns BIGINT,
    prev_hash VARCHAR, new_hash VARCHAR NOT NULL
);

-- tumult-lake/src/schema/mod.rs:perf::DDL
CREATE TABLE IF NOT EXISTS perf_verdicts (
    target VARCHAR NOT NULL, probe VARCHAR NOT NULL,
    baseline_version VARCHAR NOT NULL, candidate_version VARCHAR NOT NULL,
    quantile VARCHAR NOT NULL,
    baseline_n BIGINT NOT NULL, candidate_n BIGINT NOT NULL,
    baseline_p50 DOUBLE, baseline_p95 DOUBLE, baseline_p99 DOUBLE,
    candidate_p50 DOUBLE, candidate_p95 DOUBLE, candidate_p99 DOUBLE,
    delta DOUBLE, delta_pct DOUBLE, ci_low DOUBLE, ci_high DOUBLE,
    confidence DOUBLE NOT NULL, margin DOUBLE NOT NULL,
    verdict VARCHAR NOT NULL, reasons JSON NOT NULL,
    computed_at_ns BIGINT NOT NULL,
    envs JSON
);
ALTER TABLE perf_verdicts ADD COLUMN IF NOT EXISTS envs JSON;
CREATE INDEX IF NOT EXISTS idx_perf_verdicts_target ON perf_verdicts (target);
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS target_version VARCHAR;
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS version_source VARCHAR;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS aborted BOOLEAN;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS truncated BOOLEAN;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS planned_windows INTEGER;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS completed_windows INTEGER;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS unanswered_windows INTEGER;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS lost_to_unanswered INTEGER;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS sampled_duration_s DOUBLE;
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS version_shares VARCHAR;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS requests_sent BIGINT;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS requests_ok BIGINT;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS requests_timed_out BIGINT;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS requests_transport BIGINT;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS requests_http_4xx BIGINT;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS requests_http_5xx BIGINT;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS requests_http_other BIGINT;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS requests_unclassified BIGINT;
ALTER TABLE load_results ADD COLUMN IF NOT EXISTS censored_windows INTEGER;
CREATE TABLE IF NOT EXISTS service_version_sightings (
    service_name VARCHAR NOT NULL, environment VARCHAR,
    service_version VARCHAR NOT NULL,
    first_seen_ns BIGINT NOT NULL, last_seen_ns BIGINT NOT NULL
);
ALTER TABLE service_version_sightings ADD COLUMN IF NOT EXISTS last_deployed_ns BIGINT;
ALTER TABLE service_version_sightings ADD COLUMN IF NOT EXISTS deployments INTEGER;
ALTER TABLE service_version_sightings ADD COLUMN IF NOT EXISTS sanitized BOOLEAN;
ALTER TABLE service_version_sightings ADD COLUMN IF NOT EXISTS first_anchor_ns BIGINT;
CREATE TABLE IF NOT EXISTS sightings_backfill (
    done BOOLEAN NOT NULL, cursor_ns BIGINT
);

-- tumult-lake/src/schema/mod.rs:objectives::DR_FACTS_COLUMNS
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS impact_observed BOOLEAN;
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS outage_started_at_ns BIGINT;
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS recovered_at_ns BIGINT;
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS mttr_s DOUBLE;
ALTER TABLE nfr_verdicts ADD COLUMN IF NOT EXISTS recovered BOOLEAN;
ALTER TABLE nfr_verdicts ADD COLUMN IF NOT EXISTS reason VARCHAR;

-- tumult-lake/src/schema/mod.rs:objectives::SCENARIOS_COLUMN
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS dora_scenarios VARCHAR;

-- tumult-lake/src/schema/mod.rs:forecast::DDL
CREATE TABLE IF NOT EXISTS forecasts (
    series_key VARCHAR NOT NULL, model VARCHAR NOT NULL,
    model_version VARCHAR NOT NULL, weights_hash VARCHAR NOT NULL,
    context_start_ns BIGINT NOT NULL, context_end_ns BIGINT NOT NULL,
    context_points BIGINT NOT NULL, insufficient_history BOOLEAN NOT NULL,
    horizon BIGINT NOT NULL, step_ns BIGINT NOT NULL,
    quantile_levels DOUBLE[] NOT NULL, quantiles DOUBLE[][] NOT NULL,
    created_at_ns BIGINT NOT NULL,
    degenerate_band BOOLEAN DEFAULT FALSE
);
ALTER TABLE forecasts ADD COLUMN IF NOT EXISTS degenerate_band BOOLEAN DEFAULT FALSE;
CREATE INDEX IF NOT EXISTS idx_forecasts_series_key ON forecasts (series_key);
DROP INDEX IF EXISTS idx_forecasts_created_at;

-- tumult-lake/src/schema/mod.rs:ict::DDL
CREATE TABLE IF NOT EXISTS ict_legal_entities (
    lei VARCHAR NOT NULL,
    name VARCHAR NOT NULL,
    country VARCHAR,
    entity_type VARCHAR,
    competent_authority VARCHAR,
    profile VARCHAR NOT NULL,
    flags VARCHAR NOT NULL,
    feeds VARCHAR NOT NULL,
    registry_signed_by VARCHAR,
    registry_signed_at VARCHAR,
    updated_at_ns BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS ict_functions (
    function_id VARCHAR NOT NULL,
    register_code VARCHAR NOT NULL,
    lei VARCHAR NOT NULL,
    name VARCHAR NOT NULL,
    licensed_activity VARCHAR NOT NULL,
    tier VARCHAR NOT NULL,
    is_critical_or_important BOOLEAN NOT NULL,
    reasons VARCHAR,
    last_assessed VARCHAR,
    rto_s DOUBLE,
    rpo_s DOUBLE,
    impact VARCHAR,
    org_unit VARCHAR,
    declared VARCHAR NOT NULL,
    test_plan VARCHAR NOT NULL,
    active BOOLEAN NOT NULL,
    position BIGINT NOT NULL,
    registered_at_ns BIGINT NOT NULL,
    updated_at_ns BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS ict_function_services (
    function_id VARCHAR NOT NULL,
    kind VARCHAR NOT NULL,
    pattern VARCHAR NOT NULL,
    position BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS requirement_status_daily (
    as_of DATE NOT NULL,
    function_id VARCHAR NOT NULL,
    requirement_id VARCHAR NOT NULL,
    status VARCHAR NOT NULL,
    value DOUBLE,
    source VARCHAR NOT NULL,
    computed_at_ns BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS maturity_daily (
    as_of DATE NOT NULL,
    lei VARCHAR NOT NULL,
    level INTEGER NOT NULL,
    index_value DOUBLE NOT NULL,
    unknown_share DOUBLE NOT NULL,
    source VARCHAR NOT NULL,
    computed_at_ns BIGINT NOT NULL
);

-- tumult-lake/src/schema/mod.rs:entities::DDL
CREATE TABLE IF NOT EXISTS entity_versions (
    entity_key VARCHAR NOT NULL,
    environment VARCHAR NOT NULL DEFAULT '',
    entity_type VARCHAR NOT NULL,
    entity_id MAP (VARCHAR, VARCHAR) NOT NULL,
    state_hash VARCHAR NOT NULL,
    description JSON,
    relationships JSON,
    valid_from_ns BIGINT NOT NULL,
    last_seen_ns BIGINT NOT NULL,
    last_received_ns BIGINT NOT NULL,
    interval_s BIGINT NOT NULL DEFAULT 0,
    deleted_at_ns BIGINT,
    delete_reason VARCHAR,
    wire_form VARCHAR NOT NULL,
    observer VARCHAR NOT NULL,
    schema_url VARCHAR
);

-- tumult-lake/src/schema/mod.rs:MIGRATE_V5_RUN_TABLES_INDEX_FREE
BEGIN TRANSACTION;
CREATE TABLE runs_v5 (
    id VARCHAR NOT NULL,
    registry_id VARCHAR NOT NULL,
    state VARCHAR NOT NULL,
    params_json JSON,
    experiment_id VARCHAR,
    rollback_status VARCHAR,
    error VARCHAR,
    queued_at_ns BIGINT NOT NULL,
    started_at_ns BIGINT,
    ended_at_ns BIGINT
);
INSERT INTO runs_v5 SELECT
    id,
    registry_id,
    state,
    params_json,
    experiment_id,
    rollback_status,
    error,
    queued_at_ns,
    started_at_ns,
    ended_at_ns
FROM runs;
DROP TABLE runs;
ALTER TABLE runs_v5 RENAME TO runs;
CREATE TABLE run_registry_v5 (
    id VARCHAR NOT NULL,
    name VARCHAR NOT NULL,
    definition_toon VARCHAR NOT NULL,
    content_hash VARCHAR NOT NULL,
    registered_at_ns BIGINT NOT NULL,
    registered_by VARCHAR
);
INSERT INTO run_registry_v5 SELECT
    id,
    name,
    definition_toon,
    content_hash,
    registered_at_ns,
    registered_by
FROM run_registry;
DROP TABLE run_registry;
ALTER TABLE run_registry_v5 RENAME TO run_registry;
DROP INDEX IF EXISTS idx_run_audit_run;
COMMIT;

-- tumult-lake/src/schema/mod.rs:MIGRATE_V8_MANUAL_EXPERIMENTS_INDEX_FREE
-- Not prepared: it copies by position from manual_experiments as v2 to v7 left it.
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

-- tumult-lake/src/schema/mod.rs:manual::V22_COLUMNS
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
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS findings_severity VARCHAR;
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS tester VARCHAR;
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS tester_kind VARCHAR;
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS tlpt_attestation_ref VARCHAR;
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS tlpt_attested_at_ns BIGINT;
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS tlpt_summary_ref VARCHAR;
ALTER TABLE manual_action_items ADD COLUMN IF NOT EXISTS severity VARCHAR;
ALTER TABLE manual_action_items ADD COLUMN IF NOT EXISTS retest_kind VARCHAR;
ALTER TABLE manual_action_items ADD COLUMN IF NOT EXISTS retest_id VARCHAR;

-- tumult-lake/src/schema/mod.rs:manual::V26_COLUMNS
ALTER TABLE manual_experiments ADD COLUMN IF NOT EXISTS scenarios VARCHAR[];

-- tumult-lake/src/schema/mod.rs:entities::ADD_RECEIPT_COLUMN
ALTER TABLE entity_versions ADD COLUMN IF NOT EXISTS last_received_ns BIGINT;
UPDATE entity_versions SET last_received_ns = last_seen_ns
WHERE last_received_ns IS NULL;

-- tumult-lake/src/schema/mod.rs:run_roots_view()
CREATE OR REPLACE VIEW run_roots AS
SELECT
    s.ts_ns,
    s.trace_id,
    s.span_id,
    s.parent_span_id,
    s.span_name,
    s.span_kind,
    s.duration_ns,
    s.status_code,
    s.status_message,
    s.service_name,
    s.service_version,
    s.experiment_id,
    s.experiment_name,
    s.outcome_status,
    s.fault_type,
    s.fault_subtype,
    s.fault_severity,
    s.blast_radius,
    s.target_system,
    s.target_technology,
    s.target_environment,
    s.plugin_name,
    s.hypothesis_met,
    s.recovery_time_s,
    s.span_attrs,
    s.resource_attrs,
    s.events,
    FALSE AS journal_only
FROM spans AS s
WHERE s.span_name = 'resilience.experiment'
UNION ALL
SELECT
    e.started_at_ns AS ts_ns,
    NULL AS trace_id,
    NULL AS span_id,
    NULL AS parent_span_id,
    'resilience.experiment' AS span_name,
    'Internal' AS span_kind,
    e.ended_at_ns - e.started_at_ns AS duration_ns,
    CASE WHEN lower(e.status) = 'completed' THEN 'Ok' ELSE 'Error' END AS status_code,
    '' AS status_message,
    'tumult' AS service_name,
    NULL AS service_version,
    e.experiment_id,
    e.title AS experiment_name,
    lower(e.status) AS outcome_status,
    NULL AS fault_type,
    NULL AS fault_subtype,
    NULL AS fault_severity,
    NULL AS blast_radius,
    json_extract_string(g.attrs, '$.target_system') AS target_system,
    json_extract_string(g.attrs, '$.target_technology') AS target_technology,
    json_extract_string(g.attrs, '$.target_environment') AS target_environment,
    NULL AS plugin_name,
    coalesce(
        e.hypothesis_after_met,
        CASE WHEN NOT e.hypothesis_before_met THEN FALSE END
    ) AS hypothesis_met,
    coalesce(e.mttr_s, e.rto_measured_s) AS recovery_time_s,
    NULL AS span_attrs,
    NULL AS resource_attrs,
    NULL AS events,
    TRUE AS journal_only
FROM experiments AS e
LEFT JOIN graph_nodes AS g ON g.id = 'run:' || e.experiment_id
WHERE NOT EXISTS (
    SELECT 1 FROM spans AS r
    WHERE
        r.span_name = 'resilience.experiment'
        AND r.experiment_id = e.experiment_id
);

-- tumult-lake/src/schema/mod.rs:run_completions_view()
CREATE OR REPLACE VIEW run_completions AS
SELECT
    ts_ns,
    log_attrs,
    log_attrs['experiment_id'] AS experiment_id
FROM logs
WHERE body = 'experiment.completed'
QUALIFY
    row_number() OVER (
        PARTITION BY log_attrs['experiment_id']
        ORDER BY ts_ns DESC
    ) = 1;

-- tumult-lake/src/schema/mod.rs:experiment_runs_view()
CREATE OR REPLACE VIEW experiment_runs AS
SELECT
    s.experiment_id,
    any_value(s.experiment_name) AS experiment_name,
    min(s.ts_ns) AS started_at_ns,
    max(s.ts_ns + s.duration_ns) AS ended_at_ns,
    max(s.duration_ns) AS duration_ns,
    any_value(lower(coalesce(s.outcome_status, l.log_attrs['status']))) AS outcome_status,
    any_value(s.hypothesis_met) AS hypothesis_met
FROM run_roots AS s
LEFT JOIN run_completions AS l ON s.experiment_id = l.experiment_id
GROUP BY s.experiment_id;

-- tumult-lake/src/schema/mod.rs:entities::VIEW
CREATE OR REPLACE VIEW entities_current AS
SELECT
    entity_key,
    environment,
    entity_type,
    entity_id,
    state_hash,
    description,
    relationships,
    valid_from_ns,
    last_seen_ns,
    last_received_ns,
    interval_s,
    deleted_at_ns,
    delete_reason,
    wire_form,
    observer,
    schema_url,
    CASE
        WHEN deleted_at_ns IS NOT NULL THEN 'gone'
        WHEN
            interval_s > 0
            AND epoch_ns(now()) - last_received_ns > 3 * interval_s * 1000000000
            THEN 'expired'
        ELSE 'live'
    END AS liveness
FROM entity_versions
QUALIFY
    row_number() OVER (
        PARTITION BY entity_key, environment
        ORDER BY valid_from_ns DESC
    ) = 1;

-- tumult-lake/src/schema/mod.rs:dr::dr_tests_view(&dr_types)
CREATE OR REPLACE VIEW dr_tests AS
WITH
rto AS (
    SELECT
        experiment_id,
        bool_and(recovered) AS recovered
    FROM nfr_verdicts
    WHERE kind = 'rto' AND nfr_id = 'rto'
    GROUP BY experiment_id
),

roots AS (
    SELECT
        experiment_id,
        any_value(target_system) AS target_system,
        any_value(target_environment) AS target_environment
    FROM run_roots
    GROUP BY experiment_id
),

automated AS (
    SELECT
        'automated' AS origin,
        e.experiment_id AS evidence_id,
        e.title AS experiment_name,
        NULL AS exercise_type,
        r.target_system,
        r.target_environment,
        e.started_at_ns AS executed_at_ns,
        lower(e.status) AS outcome,
        e.rto_target_s,
        coalesce(e.rto_measured_s, e.mttr_s) AS rto_measured_s,
        coalesce(
            v.recovered,
            CASE WHEN e.mttr_s IS NOT NULL THEN TRUE END
        ) AS recovered,
        NOT e.impact_observed AS no_outage,
        e.rpo_target_s,
        e.rpo_measured_s,
        NULL AS data_verified,
        NULL AS reviewer,
        NULL AS plan_id
    FROM experiments AS e
    LEFT JOIN roots AS r ON e.experiment_id = r.experiment_id
    LEFT JOIN rto AS v ON e.experiment_id = v.experiment_id
    WHERE e.test_kind = 'dr'
),

manual AS (
    SELECT
        'manual' AS origin,
        m.id AS evidence_id,
        m.experiment_name,
        m.exercise_type,
        m.target_system,
        m.target_environment,
        m.executed_at_ns,
        m.outcome_status AS outcome,
        m.rto_target_s,
        m.recovery_time_s AS rto_measured_s,
        CASE WHEN m.recovery_time_s IS NOT NULL THEN TRUE END AS recovered,
        NULL AS no_outage,
        m.rpo_target_s,
        m.rpo_measured_s,
        m.data_verified,
        m.reviewed_by AS reviewer,
        m.plan_id
    FROM manual_experiments AS m
    WHERE
        m.status IN ('submitted', 'verified')
        AND list_contains(list_value('backup', 'drill', 'failover', 'restore'), m.exercise_type)
),

tests AS (
    SELECT * FROM automated
    UNION ALL
    SELECT * FROM manual
),

judged AS (
    SELECT
        *,
        origin || ':' || evidence_id AS test_id,
        CASE
            WHEN NOT recovered THEN 'not_recovered'
            WHEN
                rto_measured_s > rto_target_s
                OR rpo_measured_s > rpo_target_s
                THEN 'breached'
            WHEN
                coalesce(rto_target_s, rpo_target_s) IS NOT NULL
                AND (rto_target_s IS NULL OR rto_measured_s IS NOT NULL)
                AND (rpo_target_s IS NULL OR rpo_measured_s IS NOT NULL)
                THEN 'met'
            ELSE 'not_measured'
        END AS verdict
    FROM tests
)

SELECT
    test_id,
    origin,
    experiment_name,
    exercise_type,
    target_system,
    target_environment,
    executed_at_ns,
    outcome,
    rto_target_s,
    rto_measured_s,
    recovered,
    no_outage,
    rpo_target_s,
    rpo_measured_s,
    data_verified,
    verdict,
    evidence_id,
    reviewer,
    plan_id
FROM judged;

-- tumult-lake/src/schema/mod.rs:run_completions_sql(Some(1_774_980_000_000_000_000))
SELECT
    ts_ns,
    log_attrs,
    log_attrs['experiment_id'] AS experiment_id
FROM logs
WHERE
    body = 'experiment.completed'
    AND ts_ns >= 1774980000000000000
QUALIFY
    row_number() OVER (
        PARTITION BY log_attrs['experiment_id']
        ORDER BY ts_ns DESC
    ) = 1;

-- tumult-lake/src/schema/mod.rs:dr::dr_tests_sql("list_value('backup', 'restore')")
WITH
rto AS (
    SELECT
        experiment_id,
        bool_and(recovered) AS recovered
    FROM nfr_verdicts
    WHERE kind = 'rto' AND nfr_id = 'rto'
    GROUP BY experiment_id
),

roots AS (
    SELECT
        experiment_id,
        any_value(target_system) AS target_system,
        any_value(target_environment) AS target_environment
    FROM run_roots
    GROUP BY experiment_id
),

automated AS (
    SELECT
        'automated' AS origin,
        e.experiment_id AS evidence_id,
        e.title AS experiment_name,
        NULL AS exercise_type,
        r.target_system,
        r.target_environment,
        e.started_at_ns AS executed_at_ns,
        lower(e.status) AS outcome,
        e.rto_target_s,
        coalesce(e.rto_measured_s, e.mttr_s) AS rto_measured_s,
        coalesce(
            v.recovered,
            CASE WHEN e.mttr_s IS NOT NULL THEN TRUE END
        ) AS recovered,
        NOT e.impact_observed AS no_outage,
        e.rpo_target_s,
        e.rpo_measured_s,
        NULL AS data_verified,
        NULL AS reviewer,
        NULL AS plan_id
    FROM experiments AS e
    LEFT JOIN roots AS r ON e.experiment_id = r.experiment_id
    LEFT JOIN rto AS v ON e.experiment_id = v.experiment_id
    WHERE e.test_kind = 'dr'
),

manual AS (
    SELECT
        'manual' AS origin,
        m.id AS evidence_id,
        m.experiment_name,
        m.exercise_type,
        m.target_system,
        m.target_environment,
        m.executed_at_ns,
        m.outcome_status AS outcome,
        m.rto_target_s,
        m.recovery_time_s AS rto_measured_s,
        CASE WHEN m.recovery_time_s IS NOT NULL THEN TRUE END AS recovered,
        NULL AS no_outage,
        m.rpo_target_s,
        m.rpo_measured_s,
        m.data_verified,
        m.reviewed_by AS reviewer,
        m.plan_id
    FROM manual_experiments AS m
    WHERE
        m.status IN ('submitted', 'verified')
        AND list_contains(list_value('backup', 'restore'), m.exercise_type)
),

tests AS (
    SELECT * FROM automated
    UNION ALL
    SELECT * FROM manual
),

judged AS (
    SELECT
        *,
        origin || ':' || evidence_id AS test_id,
        CASE
            WHEN NOT recovered THEN 'not_recovered'
            WHEN
                rto_measured_s > rto_target_s
                OR rpo_measured_s > rpo_target_s
                THEN 'breached'
            WHEN
                coalesce(rto_target_s, rpo_target_s) IS NOT NULL
                AND (rto_target_s IS NULL OR rto_measured_s IS NOT NULL)
                AND (rpo_target_s IS NULL OR rpo_measured_s IS NOT NULL)
                THEN 'met'
            ELSE 'not_measured'
        END AS verdict
    FROM tests
)

SELECT
    test_id,
    origin,
    experiment_name,
    exercise_type,
    target_system,
    target_environment,
    executed_at_ns,
    outcome,
    rto_target_s,
    rto_measured_s,
    recovered,
    no_outage,
    rpo_target_s,
    rpo_measured_s,
    data_verified,
    verdict,
    evidence_id,
    reviewer,
    plan_id
FROM judged;

-- tumult-lake/src/schema/mod.rs:dr::dr_register_sql("list_value('backup', 'restore')", "1774980000000000000")
WITH
dr_tests AS (
    WITH
    rto AS (
        SELECT
            experiment_id,
            bool_and(recovered) AS recovered
        FROM nfr_verdicts
        WHERE kind = 'rto' AND nfr_id = 'rto'
        GROUP BY experiment_id
    ),

    roots AS (
        SELECT
            experiment_id,
            any_value(target_system) AS target_system,
            any_value(target_environment) AS target_environment
        FROM run_roots
        GROUP BY experiment_id
    ),

    automated AS (
        SELECT
            'automated' AS origin,
            e.experiment_id AS evidence_id,
            e.title AS experiment_name,
            NULL AS exercise_type,
            r.target_system,
            r.target_environment,
            e.started_at_ns AS executed_at_ns,
            lower(e.status) AS outcome,
            e.rto_target_s,
            coalesce(e.rto_measured_s, e.mttr_s) AS rto_measured_s,
            coalesce(
                v.recovered,
                CASE WHEN e.mttr_s IS NOT NULL THEN TRUE END
            ) AS recovered,
            NOT e.impact_observed AS no_outage,
            e.rpo_target_s,
            e.rpo_measured_s,
            NULL AS data_verified,
            NULL AS reviewer,
            NULL AS plan_id
        FROM experiments AS e
        LEFT JOIN roots AS r ON e.experiment_id = r.experiment_id
        LEFT JOIN rto AS v ON e.experiment_id = v.experiment_id
        WHERE e.test_kind = 'dr'
    ),

    manual AS (
        SELECT
            'manual' AS origin,
            m.id AS evidence_id,
            m.experiment_name,
            m.exercise_type,
            m.target_system,
            m.target_environment,
            m.executed_at_ns,
            m.outcome_status AS outcome,
            m.rto_target_s,
            m.recovery_time_s AS rto_measured_s,
            CASE WHEN m.recovery_time_s IS NOT NULL THEN TRUE END AS recovered,
            NULL AS no_outage,
            m.rpo_target_s,
            m.rpo_measured_s,
            m.data_verified,
            m.reviewed_by AS reviewer,
            m.plan_id
        FROM manual_experiments AS m
        WHERE
            m.status IN ('submitted', 'verified')
            AND list_contains(list_value('backup', 'restore'), m.exercise_type)
    ),

    tests AS (
        SELECT * FROM automated
        UNION ALL
        SELECT * FROM manual
    ),

    judged AS (
        SELECT
            *,
            origin || ':' || evidence_id AS test_id,
            CASE
                WHEN NOT recovered THEN 'not_recovered'
                WHEN
                    rto_measured_s > rto_target_s
                    OR rpo_measured_s > rpo_target_s
                    THEN 'breached'
                WHEN
                    coalesce(rto_target_s, rpo_target_s) IS NOT NULL
                    AND (rto_target_s IS NULL OR rto_measured_s IS NOT NULL)
                    AND (rpo_target_s IS NULL OR rpo_measured_s IS NOT NULL)
                    THEN 'met'
                ELSE 'not_measured'
            END AS verdict
        FROM tests
    )

    SELECT
        test_id,
        origin,
        experiment_name,
        exercise_type,
        target_system,
        target_environment,
        executed_at_ns,
        outcome,
        rto_target_s,
        rto_measured_s,
        recovered,
        no_outage,
        rpo_target_s,
        rpo_measured_s,
        data_verified,
        verdict,
        evidence_id,
        reviewer,
        plan_id
    FROM judged
),

counted AS (
    SELECT
        target_system,
        target_environment,
        test_id,
        executed_at_ns
    FROM dr_tests
    WHERE
        target_system IS NOT NULL
        AND executed_at_ns <= 1774980000000000000
        AND outcome NOT IN ('aborted', 'interrupted', 'inconclusive')
        AND NOT coalesce(no_outage, FALSE)
),

last_tests AS (
    SELECT
        target_system,
        target_environment,
        max(executed_at_ns) AS last_test_at_ns,
        arg_max(test_id, executed_at_ns) AS last_test_id
    FROM counted
    GROUP BY target_system, target_environment
),

dr_plans AS (
    SELECT
        id,
        target_system,
        target_environment,
        interval_days,
        due_at_ns
    FROM manual_exercise_plans
    WHERE
        status = 'planned'
        AND target_system IS NOT NULL
        AND list_contains(list_value('backup', 'restore'), exercise_type)
),

declared AS (
    SELECT
        r.target_system,
        r.target_environment
    FROM experiments AS e
    INNER JOIN run_roots AS r ON e.experiment_id = r.experiment_id
    WHERE e.rto_target_s IS NOT NULL
    UNION
    SELECT
        target_system,
        target_environment
    FROM manual_experiments
    WHERE rto_target_s IS NOT NULL AND status IN ('submitted', 'verified')
    UNION
    SELECT
        target_system,
        target_environment
    FROM manual_exercise_plans
    WHERE rto_target_s IS NOT NULL AND status = 'planned'
),

seen AS (
    SELECT
        target_system,
        target_environment
    FROM dr_tests
    UNION
    SELECT
        target_system,
        target_environment
    FROM declared
),

targets AS (
    SELECT
        target_system,
        target_environment
    FROM seen
    WHERE target_system IS NOT NULL
    UNION
    SELECT
        p.target_system,
        p.target_environment
    FROM dr_plans AS p
    WHERE NOT EXISTS (
        SELECT 1 FROM seen AS s
        WHERE
            s.target_system = p.target_system
            AND (
                p.target_environment IS NULL
                OR s.target_environment = p.target_environment
            )
    )
),

covering_plans AS (
    SELECT
        t.target_system,
        t.target_environment,
        arg_min(p.id, p.due_at_ns) AS plan_id,
        min(p.due_at_ns) AS plan_due_at_ns,
        min(p.interval_days) AS interval_days
    FROM targets AS t
    INNER JOIN dr_plans AS p
        ON
            t.target_system = p.target_system
            AND (
                p.target_environment IS NULL
                OR t.target_environment = p.target_environment
            )
    GROUP BY t.target_system, t.target_environment
),

planned_tests AS (
    SELECT
        function_id,
        unnest(
            from_json(test_plan, '[{"kind": "VARCHAR", "interval_days": "BIGINT"}]')
        ) AS planned
    FROM ict_functions
    WHERE active
),

function_intervals AS (
    SELECT
        function_id,
        min(
            least(struct_extract(planned, 'interval_days'), 3650)
        ) AS interval_days
    FROM planned_tests
    WHERE
        struct_extract(planned, 'kind') = 'dr'
        AND struct_extract(planned, 'interval_days') > 0
    GROUP BY function_id
),

target_names AS (
    SELECT DISTINCT target_system FROM targets
),

service_patterns AS (
    SELECT
        function_id,
        kind,
        pattern,
        replace(
            replace(replace(replace(pattern, '!', '!!'), '%', '!%'), '_', '!_'),
            '*',
            '%'
        ) AS like_pattern
    FROM ict_function_services
),

bound_functions AS (
    SELECT
        t.target_system,
        f.function_id
    FROM target_names AS t
    INNER JOIN function_intervals AS f ON t.target_system = f.function_id
    UNION
    SELECT
        t.target_system,
        s.function_id
    FROM target_names AS t
    INNER JOIN service_patterns AS s
        ON
            (s.kind = 'entity' AND t.target_system = s.pattern)
            OR (
                s.kind = 'service'
                AND t.target_system LIKE s.like_pattern ESCAPE '!'
            )
),

target_intervals AS (
    SELECT
        b.target_system,
        min(f.interval_days) AS interval_days
    FROM bound_functions AS b
    INNER JOIN function_intervals AS f ON b.function_id = f.function_id
    GROUP BY b.target_system
),

due_rows AS (
    SELECT
        t.target_system,
        t.target_environment,
        l.last_test_at_ns,
        l.last_test_id,
        c.plan_id,
        c.plan_due_at_ns,
        coalesce(c.interval_days, i.interval_days, 365) AS interval_days,
        CASE
            WHEN c.interval_days IS NOT NULL THEN 'plan'
            WHEN i.interval_days IS NOT NULL THEN 'function'
            ELSE 'default'
        END AS interval_source
    FROM targets AS t
    LEFT JOIN last_tests AS l
        ON
            t.target_system = l.target_system
            AND t.target_environment IS NOT DISTINCT FROM l.target_environment
    LEFT JOIN covering_plans AS c
        ON
            t.target_system = c.target_system
            AND t.target_environment IS NOT DISTINCT FROM c.target_environment
    LEFT JOIN target_intervals AS i ON t.target_system = i.target_system
)

SELECT
    target_system,
    target_environment,
    last_test_at_ns,
    last_test_id,
    interval_days,
    interval_source,
    plan_id,
    plan_due_at_ns,
    last_test_at_ns + interval_days * 86400000000000 AS next_due_at_ns,
    CASE
        WHEN last_test_at_ns IS NULL THEN 'never_tested'
        WHEN last_test_at_ns + interval_days * 86400000000000 < 1774980000000000000 THEN 'overdue'
        WHEN
            last_test_at_ns + interval_days * 86400000000000 - 1774980000000000000
            <= 2592000000000000
            THEN 'due'
        ELSE 'ok'
    END AS state
FROM due_rows;

-- tumult-lake/src/schema/mod.rs:run_outcome_sql("s", "l")
SELECT lower(coalesce(s.outcome_status, l.log_attrs['status'])) AS outcome FROM run_roots AS s LEFT JOIN run_completions AS l ON s.experiment_id = l.experiment_id;

-- tumult-lake/src/schema/mod.rs:run_verdict_sql("s.outcome_status")
SELECT CASE lower(s.outcome_status)
        WHEN 'completed' THEN 'pass'
        WHEN 'deviated' THEN 'fail'
        WHEN 'failed' THEN 'fail'
        WHEN 'halted' THEN 'fail'
        WHEN 'aborted' THEN 'no_evidence'
        WHEN 'interrupted' THEN 'no_evidence'
    END AS verdict FROM run_roots AS s;

-- tumult-lake/src/schema/mod.rs:mean_recovery_time_sql("s")
SELECT avg(s.recovery_time_s) AS mean_recovery_time_s FROM run_roots AS s;

-- tumult-lake/src/schema/mod.rs:hypothesis_met_sql("e")
SELECT coalesce(
        e.hypothesis_after_met,
        CASE WHEN NOT e.hypothesis_before_met THEN FALSE END
    ) AS hypothesis_met FROM experiments AS e;

-- tumult-lake/src/sql/analytics.rs:EXPERIMENT_EXISTS
SELECT count(*) FROM experiments WHERE experiment_id = ?;

-- tumult-lake/src/sql/analytics.rs:AGENTIC_RUN_EXISTS
SELECT count(*) FROM agentic_runs WHERE run_id = ?;

-- tumult-lake/src/sql/analytics.rs:INSERT_AGENTIC_RUN
INSERT INTO agentic_runs (
        run_id, experiment_id, target_type, scenario,
        resilience_score, trace_id, replay_id
    ) VALUES (?, ?, ?, ?, ?, ?, ?);

-- tumult-lake/src/sql/analytics.rs:INSERT_CONTRACT_OUTCOME
INSERT INTO agentic_contract_outcomes (
        run_id, scenario, contract_type, passed, reason, severity
    ) VALUES (?, ?, ?, ?, ?, ?);

-- tumult-lake/src/sql/analytics.rs:INSERT_FAULT_APPLICATION
INSERT INTO agentic_fault_applications (
        run_id, scenario, fault_type, applied
    ) VALUES (?, ?, ?, ?);

-- tumult-lake/src/sql/analytics.rs:INSERT_REPLAY_OUTCOME
INSERT INTO agentic_replay_outcomes (
        run_id, replay_id, scenario, passed
    ) VALUES (?, ?, ?, ?);

-- tumult-lake/src/sql/analytics.rs:PURGE_EXPERIMENTS
DELETE FROM experiments WHERE started_at_ns < ? RETURNING experiment_id;

-- tumult-lake/src/sql/analytics.rs:ACTIVITIES_EXPORT
SELECT experiment_id, name, activity_type, status, started_at_ns, duration_ms, output, error, phase FROM activity_results;

-- tumult-lake/src/sql/analytics.rs:DENY_EXTERNAL_ACCESS
SET enable_external_access = false;

-- tumult-lake/src/sql/analytics.rs:INSERT_AUTOPILOT_DECISION
INSERT INTO autopilot_decisions VALUES
    (?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS JSON), ?, ?, CAST(? AS JSON), ?,
     CAST(? AS JSON), CAST(? AS JSON), ?, ?);

-- tumult-lake/src/sql/analytics.rs:INSERT_AUTOPILOT_EVENT
INSERT INTO autopilot_events VALUES (?, ?, ?, CAST(? AS JSON));

-- tumult-lake/src/sql/analytics.rs:INSERT_CHANGE_EVENT
INSERT INTO autopilot_change_events VALUES (?, ?, ?, ?);

-- tumult-lake/src/sql/analytics.rs:CURRENT_DATABASE
SELECT current_database();

-- tumult-lake/src/sql/analytics.rs:TABLE_COLUMNS
SELECT column_name FROM information_schema.columns
    WHERE table_catalog = ? AND table_name = ? ORDER BY ordinal_position;

-- tumult-lake/src/sql/analytics.rs:COUNT_EXPERIMENTS
SELECT count(*) FROM experiments;

-- tumult-lake/src/sql/analytics.rs:COUNT_ACTIVITIES
SELECT count(*) FROM activity_results;

-- tumult-lake/src/sql/approvals.rs:INSERT_REQUEST
INSERT INTO approval_requests (run_id, tier, pin_hash, env, target, quorum_required, requested_by, requested_at_ns, expires_at_ns) VALUES (?,?,?,?,?,?,?,?,?);

-- tumult-lake/src/sql/approvals.rs:INSERT_DECISION
INSERT INTO approval_decisions (run_id, approver, decision, note, decided_at_ns) VALUES (?,?,?,?,?);

-- tumult-lake/src/sql/approvals.rs:COUNT_PENDING_RUN
SELECT COUNT(*) FROM runs WHERE id = ? AND state = ?;

-- tumult-lake/src/sql/approvals.rs:CONSUME
UPDATE approval_requests SET consumed_at_ns = ? WHERE run_id = ?;

-- tumult-lake/src/sql/approvals.rs:BREAK_GLASS
UPDATE approval_requests SET break_glass = TRUE, break_glass_by = ?, break_glass_justification = ? WHERE run_id = ?;

-- tumult-lake/src/sql/approvals.rs:QUEUE
SELECT r.id AS run_id, r.state, r.queued_at_ns, r.params_json, g.name AS definition_name, q.*, (SELECT COUNT(*) FROM approval_decisions d WHERE d.run_id = r.id AND d.decision = 'approved') AS approved_count FROM runs r JOIN approval_requests q ON q.run_id = r.id LEFT JOIN run_registry g ON g.id = r.registry_id WHERE r.state = 'pending_approval' ORDER BY q.requested_at_ns;

-- tumult-lake/src/sql/approvals.rs:requester("run-1")
SELECT requested_by FROM approval_requests WHERE run_id = 'run-1';

-- tumult-lake/src/sql/approvals.rs:decision_of("run-1", "ada")
SELECT approver FROM approval_decisions WHERE run_id = 'run-1' AND approver = 'ada';

-- tumult-lake/src/sql/approvals.rs:request("run-1")
SELECT * FROM approval_requests WHERE run_id = 'run-1';

-- tumult-lake/src/sql/approvals.rs:decisions("run-1")
SELECT * FROM approval_decisions WHERE run_id = 'run-1' ORDER BY decided_at_ns;

-- tumult-lake/src/sql/approvals.rs:requests(50)
SELECT q.*, g.name AS definition_name, r.state AS run_state, (SELECT COUNT(*) FROM approval_decisions d WHERE d.run_id = q.run_id AND d.decision = 'approved') AS approved_count, (SELECT COUNT(*) FROM approval_decisions d WHERE d.run_id = q.run_id AND d.decision = 'rejected') AS rejected_count FROM approval_requests q LEFT JOIN runs r ON r.id = q.run_id LEFT JOIN run_registry g ON g.id = r.registry_id ORDER BY q.requested_at_ns DESC LIMIT 50;

-- tumult-lake/src/sql/approvals.rs:expired_pending(1_774_980_000_000_000_000)
SELECT r.id AS run_id FROM runs r JOIN approval_requests q ON q.run_id = r.id WHERE r.state = 'pending_approval' AND q.expires_at_ns < 1774980000000000000;

-- tumult-lake/src/sql/approvals.rs:audit_chain("run-1")
SELECT at_ns, event, detail, actor, prev_hash, new_hash FROM run_audit WHERE run_id = 'run-1' ORDER BY at_ns, rowid;

-- tumult-lake/src/sql/auth.rs:INSERT_USER
INSERT INTO users VALUES (?,?,?,?,?,?,?);

-- tumult-lake/src/sql/auth.rs:SET_PASSWORD
UPDATE users SET password_hash = ?, must_change = false WHERE id = ?;

-- tumult-lake/src/sql/auth.rs:RESET_PASSWORD
UPDATE users SET password_hash = ?, must_change = true WHERE id = ?;

-- tumult-lake/src/sql/auth.rs:SET_ROLE
UPDATE users SET role = ? WHERE id = ?;

-- tumult-lake/src/sql/auth.rs:SET_DISABLED
UPDATE users SET disabled = ? WHERE id = ?;

-- tumult-lake/src/sql/auth.rs:INSERT_SESSION
INSERT INTO sessions VALUES (?,?,?,?);

-- tumult-lake/src/sql/auth.rs:DELETE_SESSION
DELETE FROM sessions WHERE id_hash = ?;

-- tumult-lake/src/sql/auth.rs:DELETE_USER_SESSIONS
DELETE FROM sessions WHERE user_id = ?;

-- tumult-lake/src/sql/auth.rs:INSERT_TOKEN
INSERT INTO tokens VALUES (?,?,?,?,?,?,?,?);

-- tumult-lake/src/sql/auth.rs:REVOKE_TOKEN
UPDATE tokens SET revoked = true WHERE id = ?;

-- tumult-lake/src/sql/auth.rs:REVOKE_USER_TOKENS
UPDATE tokens SET revoked = true WHERE user_id = ?;

-- tumult-lake/src/sql/auth.rs:TOUCH_TOKEN
UPDATE tokens SET last_used_at_ns = ? WHERE token_hash = ?;

-- tumult-lake/src/sql/auth.rs:DELETE_ENV_SCOPES
DELETE FROM user_env_scopes WHERE user_id = ?;

-- tumult-lake/src/sql/auth.rs:INSERT_ENV_SCOPE
INSERT INTO user_env_scopes VALUES (?,?);

-- tumult-lake/src/sql/auth.rs:COUNT_USERS
SELECT count(*) AS c FROM users;

-- tumult-lake/src/sql/auth.rs:COUNT_REAL_USERS
SELECT count(*) AS c FROM users WHERE id != 'legacy';

-- tumult-lake/src/sql/auth.rs:USERS
SELECT * FROM users ORDER BY username;

-- tumult-lake/src/sql/auth.rs:TOKENS
SELECT * FROM tokens ORDER BY created_at_ns DESC, id DESC;

-- tumult-lake/src/sql/auth.rs:user_by_username("ada")
SELECT * FROM users WHERE username = 'ada';

-- tumult-lake/src/sql/auth.rs:user_by_id("user-1")
SELECT * FROM users WHERE id = 'user-1';

-- tumult-lake/src/sql/auth.rs:session_with_user("hash", 1_774_980_000_000_000_000)
SELECT s.id_hash, s.user_id, s.created_at_ns, s.expires_at_ns, u.id AS u_id, u.username AS u_username, u.password_hash AS u_password_hash, u.role AS u_role, u.must_change AS u_must_change, u.disabled AS u_disabled, u.created_at_ns AS u_created_at_ns FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.id_hash = 'hash' AND s.expires_at_ns > 1774980000000000000;

-- tumult-lake/src/sql/auth.rs:token_with_user("hash", 1_774_980_000_000_000_000)
SELECT t.id, t.user_id, t.name, t.token_hash, t.created_at_ns, t.last_used_at_ns, t.revoked, t.expires_at_ns, u.id AS u_id, u.username AS u_username, u.password_hash AS u_password_hash, u.role AS u_role, u.must_change AS u_must_change, u.disabled AS u_disabled, u.created_at_ns AS u_created_at_ns FROM tokens t JOIN users u ON u.id = t.user_id WHERE t.token_hash = 'hash' AND t.revoked = false AND (t.expires_at_ns IS NULL OR t.expires_at_ns > 1774980000000000000);

-- tumult-lake/src/sql/auth.rs:env_scopes("user-1")
SELECT environment FROM user_env_scopes WHERE user_id = 'user-1' ORDER BY environment;

-- tumult-lake/src/sql/entities.rs:COUNT_VERSIONS
SELECT count(*) FROM entity_versions;

-- tumult-lake/src/sql/forecast.rs:PRUNE
DELETE FROM forecasts WHERE rowid IN (SELECT rowid FROM forecasts WHERE created_at_ns < ? ORDER BY created_at_ns, rowid LIMIT ?);

-- tumult-lake/src/sql/forecast.rs:CAP
DELETE FROM forecasts WHERE rowid IN (SELECT rowid FROM forecasts WHERE series_key = ? ORDER BY created_at_ns DESC, rowid DESC OFFSET ?);

-- tumult-lake/src/sql/forecast.rs:insert("[0.1, 0.5, 0.9]", "[[1.0, 2.0, 3.0]]")
INSERT INTO forecasts (series_key, model, model_version, weights_hash, context_start_ns, context_end_ns, context_points, insufficient_history, degenerate_band, horizon, step_ns, quantile_levels, quantiles, created_at_ns) VALUES (?,?,?,?,?,?,?,?,?,?,?, CAST([0.1, 0.5, 0.9] AS DOUBLE[]), CAST([[1.0, 2.0, 3.0]] AS DOUBLE[][]), ?);

-- tumult-lake/src/sql/forecast.rs:latest("=")
SELECT row_to_json(t) AS j FROM (SELECT series_key, model, model_version, weights_hash, context_start_ns, context_end_ns, context_points, insufficient_history, degenerate_band, horizon, step_ns, quantile_levels, quantiles, created_at_ns FROM forecasts WHERE series_key = ? AND model = ? AND step_ns = ? AND horizon = ? AND context_start_ns = ? AND context_end_ns = ? ORDER BY created_at_ns DESC, rowid DESC LIMIT 1) AS t;

-- tumult-lake/src/sql/forecast.rs:latest("<>")
SELECT row_to_json(t) AS j FROM (SELECT series_key, model, model_version, weights_hash, context_start_ns, context_end_ns, context_points, insufficient_history, degenerate_band, horizon, step_ns, quantile_levels, quantiles, created_at_ns FROM forecasts WHERE series_key = ? AND model <> ? AND step_ns = ? AND horizon = ? AND context_start_ns = ? AND context_end_ns = ? ORDER BY created_at_ns DESC, rowid DESC LIMIT 1) AS t;

-- tumult-lake/src/sql/ict.rs:LEGAL_ENTITIES
SELECT lei, name, country, entity_type, competent_authority, profile, flags, feeds, registry_signed_by, registry_signed_at FROM ict_legal_entities ORDER BY lei;

-- tumult-lake/src/sql/ict.rs:FUNCTION_SERVICES
SELECT function_id, kind, pattern FROM ict_function_services ORDER BY position;

-- tumult-lake/src/sql/ict.rs:DELETE_LEGAL_ENTITIES
DELETE FROM ict_legal_entities;

-- tumult-lake/src/sql/ict.rs:INSERT_LEGAL_ENTITY
INSERT INTO ict_legal_entities (lei, name, country, entity_type, competent_authority, profile, flags, feeds, registry_signed_by, registry_signed_at, updated_at_ns) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);

-- tumult-lake/src/sql/ict.rs:DEACTIVATE_FUNCTIONS
UPDATE ict_functions SET active = false, updated_at_ns = ? WHERE active;

-- tumult-lake/src/sql/ict.rs:UPDATE_FUNCTION
UPDATE ict_functions SET name = ?, tier = ?, is_critical_or_important = ?, reasons = ?, last_assessed = ?, rto_s = ?, rpo_s = ?, impact = ?, org_unit = ?, declared = ?, test_plan = ?, active = true, position = ?, updated_at_ns = ? WHERE function_id = ? AND lei = ? AND licensed_activity = ?;

-- tumult-lake/src/sql/ict.rs:INSERT_FUNCTION
INSERT INTO ict_functions (function_id, register_code, lei, name, licensed_activity, tier, is_critical_or_important, reasons, last_assessed, rto_s, rpo_s, impact, org_unit, declared, test_plan, active, position, registered_at_ns, updated_at_ns) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, true, ?, ?, ?);

-- tumult-lake/src/sql/ict.rs:DELETE_FUNCTION_SERVICES
DELETE FROM ict_function_services;

-- tumult-lake/src/sql/ict.rs:INSERT_FUNCTION_SERVICE
INSERT INTO ict_function_services (function_id, kind, pattern, position) VALUES (?, ?, ?, ?);

-- tumult-lake/src/sql/ict.rs:DELETE_REQUIREMENT_STATUS
DELETE FROM requirement_status_daily WHERE as_of = DATE '1970-01-01' + CAST(? AS INTEGER) AND function_id = ? AND requirement_id = ?;

-- tumult-lake/src/sql/ict.rs:INSERT_REQUIREMENT_STATUS
INSERT INTO requirement_status_daily (as_of, function_id, requirement_id, status, value, source, computed_at_ns) VALUES (DATE '1970-01-01' + CAST(? AS INTEGER), ?, ?, ?, ?, ?, ?);

-- tumult-lake/src/sql/ict.rs:DELETE_MATURITY
DELETE FROM maturity_daily WHERE as_of = DATE '1970-01-01' + CAST(? AS INTEGER) AND lei = ?;

-- tumult-lake/src/sql/ict.rs:INSERT_MATURITY
INSERT INTO maturity_daily (as_of, lei, level, index_value, unknown_share, source, computed_at_ns) VALUES (DATE '1970-01-01' + CAST(? AS INTEGER), ?, ?, ?, ?, ?, ?);

-- tumult-lake/src/sql/ict.rs:status_between(20_500, 20_530, 1_000)
SELECT * FROM (SELECT CAST(datediff('day', DATE '1970-01-01', as_of) AS BIGINT) AS as_of_day, function_id, requirement_id, status, value, source FROM requirement_status_daily WHERE as_of BETWEEN DATE '1970-01-01' + CAST(20500 AS INTEGER) AND DATE '1970-01-01' + CAST(20530 AS INTEGER) ORDER BY as_of DESC, function_id DESC, requirement_id DESC LIMIT 1000) AS newest ORDER BY as_of_day, function_id, requirement_id;

-- tumult-lake/src/sql/ict.rs:maturity_between(20_500, 20_530)
SELECT CAST(datediff('day', DATE '1970-01-01', as_of) AS BIGINT) AS as_of_day, lei, level, index_value, unknown_share, source FROM maturity_daily WHERE lei = ? AND as_of BETWEEN DATE '1970-01-01' + CAST(20500 AS INTEGER) AND DATE '1970-01-01' + CAST(20530 AS INTEGER) ORDER BY as_of;

-- tumult-lake/src/sql/ict.rs:prune_status(20_400)
DELETE FROM requirement_status_daily WHERE as_of < DATE '1970-01-01' + CAST(20400 AS INTEGER);

-- tumult-lake/src/sql/ict.rs:prune_maturity(20_400)
DELETE FROM maturity_daily WHERE as_of < DATE '1970-01-01' + CAST(20400 AS INTEGER);

-- tumult-lake/src/sql/manual.rs:INSERT_SIGNOFF
INSERT INTO manual_signoffs (
       id, subject_id, subject_kind, role, signed_by, signed_at_ns, decision, note,
       signed_hash, prev_hash, link_hash
    ) VALUES (?,?,?,?,?,?,?,?,?,?,?);

-- tumult-lake/src/sql/manual.rs:RECORD_ROW
SELECT row_to_json(t) AS j FROM (SELECT * FROM manual_experiments WHERE id = ?) AS t;

-- tumult-lake/src/sql/manual.rs:PLAN_ROW
SELECT row_to_json(t) AS j FROM (SELECT * FROM manual_exercise_plans WHERE id = ?) AS t;

-- tumult-lake/src/sql/manual.rs:RESEAL_RECORD
UPDATE manual_experiments SET content_hash = ?, hash_version = ? WHERE id = ?;

-- tumult-lake/src/sql/manual.rs:RESEAL_PLAN
UPDATE manual_exercise_plans SET content_hash = ? WHERE id = ?;

-- tumult-lake/src/sql/manual.rs:INSERT_PLAN
INSERT INTO manual_exercise_plans (
       id, name, exercise_type, target_system, target_environment, function_id,
       scope, owner, planned_at_ns, due_at_ns, interval_days, rto_target_s,
       rpo_target_s, status, created_by, created_at_ns, content_hash
    ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?);

-- tumult-lake/src/sql/manual.rs:CLOSE_PLAN
UPDATE manual_exercise_plans SET status = ?, updated_by = ?, updated_at_ns = ? WHERE id = ?;

-- tumult-lake/src/sql/manual.rs:LINKED_FUNCTIONS
SELECT linked_id FROM manual_experiment_links WHERE experiment_id = ? AND kind = 'function' ORDER BY ordinal;

-- tumult-lake/src/sql/manual.rs:UPDATE_PLAN
UPDATE manual_exercise_plans SET
       name = ?, exercise_type = ?, target_system = ?, target_environment = ?,
       function_id = ?, scope = ?, owner = ?, planned_at_ns = ?, due_at_ns = ?,
       interval_days = ?, rto_target_s = ?, rpo_target_s = ?,
       updated_by = ?, updated_at_ns = ?
    WHERE id = ?;

-- tumult-lake/src/sql/manual.rs:PLAN
SELECT * FROM manual_exercise_plans WHERE id = ?;

-- tumult-lake/src/sql/manual.rs:PLAN_AUDIT
SELECT * FROM manual_plan_audit WHERE plan_id = ? ORDER BY changed_at_ns ASC, id ASC;

-- tumult-lake/src/sql/manual.rs:RECORDS_WITH_STATUS
SELECT * FROM manual_experiments WHERE status = ? ORDER BY entered_at_ns DESC;

-- tumult-lake/src/sql/manual.rs:RECORDS
SELECT * FROM manual_experiments ORDER BY entered_at_ns DESC;

-- tumult-lake/src/sql/manual.rs:RECORD
SELECT * FROM manual_experiments WHERE id = ?;

-- tumult-lake/src/sql/manual.rs:RECORD_AUDIT
SELECT * FROM manual_experiment_audit WHERE experiment_id = ? ORDER BY changed_at_ns ASC, id ASC;

-- tumult-lake/src/sql/manual.rs:ATTACHMENTS
SELECT * FROM evidence_attachments WHERE experiment_id = ? ORDER BY added_at_ns ASC;

-- tumult-lake/src/sql/manual.rs:ATTACHMENTS_IN_ORDER
SELECT * FROM evidence_attachments WHERE experiment_id = ? ORDER BY added_at_ns, id;

-- tumult-lake/src/sql/manual.rs:INSERT_STEP
INSERT INTO manual_exercise_steps (
       experiment_id, ordinal, title, actor, started_at_ns, ended_at_ns, result, note
    ) VALUES (?,?,?,?,?,?,?,?);

-- tumult-lake/src/sql/manual.rs:INSERT_ACTION
INSERT INTO manual_action_items (
       id, experiment_id, ordinal, title, owner, due_at_ns, ticket,
       severity, retest_kind, retest_id
    ) VALUES (?,?,?,?,?,?,?,?,?,?);

-- tumult-lake/src/sql/manual.rs:INSERT_LINK
INSERT INTO manual_experiment_links (
       experiment_id, ordinal, kind, linked_id, label
    ) VALUES (?,?,?,?,?);

-- tumult-lake/src/sql/manual.rs:INSERT_DRAFT
INSERT INTO manual_experiments (
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
              ?,?,?,?,?,?, CAST(? AS VARCHAR[]));

-- tumult-lake/src/sql/manual.rs:UPDATE_DRAFT
UPDATE manual_experiments SET
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
    WHERE id = ?;

-- tumult-lake/src/sql/manual.rs:SUBMIT
UPDATE manual_experiments SET status = 'submitted', attestation = ? WHERE id = ?;

-- tumult-lake/src/sql/manual.rs:COUNT_APPROVALS
SELECT COUNT(*) FROM manual_signoffs WHERE subject_kind = 'record' AND subject_id = ? AND decision = 'approved';

-- tumult-lake/src/sql/manual.rs:REVIEW
UPDATE manual_experiments SET status = ?, reviewed_by = ?, reviewed_at_ns = ?, review_note = ? WHERE id = ?;

-- tumult-lake/src/sql/manual.rs:COUNT_AUTHORED
SELECT COUNT(*) FROM manual_experiment_audit WHERE experiment_id = ? AND changed_by = ? AND action IN ('create', 'edit', 'submit', 'attach');

-- tumult-lake/src/sql/manual.rs:COUNT_SIGNED
SELECT COUNT(*) FROM manual_signoffs WHERE subject_kind = 'record' AND subject_id = ? AND signed_by = ?;

-- tumult-lake/src/sql/manual.rs:INSERT_ATTACHMENT
INSERT INTO evidence_attachments (
       id, experiment_id, kind, uri, label, file_hash, added_by, added_at_ns
    ) VALUES (?,?,?,?,?,?,?,?);

-- tumult-lake/src/sql/manual.rs:INSERT_FILE
INSERT INTO evidence_attachments (
       id, experiment_id, kind, uri, label, file_hash, added_by, added_at_ns,
       media_type, size_bytes
    ) VALUES (?,?,?,?,?,?,?,?,?,?);

-- tumult-lake/src/sql/manual.rs:INSERT_IMPORT_BATCH
INSERT INTO import_batches (id, source, imported_at_ns, rows, label) VALUES (?,?,?,?,?);

-- tumult-lake/src/sql/perf.rs:INSERT_VERDICT
INSERT INTO perf_verdicts (
       target, probe, baseline_version, candidate_version, quantile,
       baseline_n, candidate_n,
       baseline_p50, baseline_p95, baseline_p99,
       candidate_p50, candidate_p95, candidate_p99,
       delta, delta_pct, ci_low, ci_high, confidence, margin,
       verdict, reasons, computed_at_ns, envs
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?);

-- tumult-lake/src/sql/perf.rs:VERDICTS
SELECT target, probe, baseline_version, candidate_version, quantile,
           baseline_n, candidate_n,
           baseline_p50, baseline_p95, baseline_p99,
           candidate_p50, candidate_p95, candidate_p99,
           delta, delta_pct, ci_low, ci_high, confidence, margin,
           verdict, CAST(reasons AS VARCHAR), computed_at_ns,
           CAST(envs AS VARCHAR)
    FROM perf_verdicts WHERE target = ?
      AND (? OR (envs IS NOT NULL AND list_has_all(
           CAST(CAST(? AS JSON) AS VARCHAR[]), CAST(envs AS VARCHAR[]))))
    ORDER BY computed_at_ns DESC, rowid DESC LIMIT ?;

-- tumult-lake/src/sql/reader.rs:EXPERIMENT_RUNS
SELECT experiment_id, experiment_name, started_at_ns, ended_at_ns,
           duration_ns, outcome_status, hypothesis_met
    FROM experiment_runs ORDER BY started_at_ns;

-- tumult-lake/src/sql/risk.rs:LAST_SUPPRESSION_HASH
SELECT new_hash FROM risk_suppressions WHERE risk_id = ? ORDER BY at_ns DESC, rowid DESC LIMIT 1;

-- tumult-lake/src/sql/risk.rs:INSERT_SUPPRESSION
INSERT INTO risk_suppressions (id, risk_id, reason, actor, at_ns, expires_at_ns, prev_hash, new_hash) VALUES (?,?,?,?,?,?,?,?);

-- tumult-lake/src/sql/risk.rs:SUPPRESSIONS
SELECT * FROM risk_suppressions ORDER BY at_ns ASC, rowid ASC;

-- tumult-lake/src/sql/runs.rs:INSERT_RUN
INSERT INTO runs (id, registry_id, state, params_json, queued_at_ns) VALUES (?,?,?,CAST(? AS JSON),?);

-- tumult-lake/src/sql/runs.rs:SET_GAMEDAY
UPDATE runs SET gameday_id = ? WHERE id = ?;

-- tumult-lake/src/sql/runs.rs:INSERT_DEFINITION
INSERT INTO run_registry (id, name, definition_toon, content_hash, registered_at_ns, registered_by) VALUES (?,?,?,?,?,?);

-- tumult-lake/src/sql/runs.rs:INSERT_GAMEDAY_DEFINITION
INSERT INTO run_registry (id, name, definition_toon, content_hash, registered_at_ns, registered_by, kind) VALUES (?,?,?,?,?,?,'gameday');

-- tumult-lake/src/sql/runs.rs:INSERT_EXECUTION_PIN
INSERT INTO run_execution_pins (run_id, execution_hash) VALUES (?, ?);

-- tumult-lake/src/sql/runs.rs:SET_STATE
UPDATE runs SET state = ? WHERE id = ? AND state NOT IN ('passed','deviated','failed','aborted','rejected','expired') AND (? <> 'queued' OR state = 'pending_approval');

-- tumult-lake/src/sql/runs.rs:MARK_STARTED
UPDATE runs SET state = ?, started_at_ns = ?, experiment_id = COALESCE(?, experiment_id) WHERE id = ? AND state IN ('queued', 'validating');

-- tumult-lake/src/sql/runs.rs:CLAIM
UPDATE runs SET state = 'validating' WHERE id = ? AND state = 'queued';

-- tumult-lake/src/sql/runs.rs:BEGIN_EXECUTION
UPDATE runs SET state = 'running', started_at_ns = ? WHERE id = ? AND state = 'validating';

-- tumult-lake/src/sql/runs.rs:CONSUME_PINNED_APPROVAL
UPDATE approval_requests SET consumed_at_ns = ? WHERE run_id = ? AND pin_hash = ? AND consumed_at_ns IS NULL AND (break_glass = TRUE OR expires_at_ns >= ?);

-- tumult-lake/src/sql/runs.rs:INSERT_RECOVERY_PLAN
INSERT INTO run_recovery_plans (run_id, plan_json) VALUES (?, CAST(? AS JSON));

-- tumult-lake/src/sql/runs.rs:REQUEST_STOP
UPDATE runs SET state = CASE WHEN started_at_ns IS NULL THEN 'aborted' ELSE 'stopping' END, ended_at_ns = CASE WHEN started_at_ns IS NULL THEN ? ELSE ended_at_ns END, rollback_status = CASE WHEN started_at_ns IS NULL THEN 'not_needed' ELSE rollback_status END, error = CASE WHEN started_at_ns IS NULL THEN 'cancelled before start' ELSE error END WHERE id = ? AND state IN ('queued','validating','running','stopping','pending_approval');

-- tumult-lake/src/sql/runs.rs:STATE
SELECT state FROM runs WHERE id = ?;

-- tumult-lake/src/sql/runs.rs:FAIL
UPDATE runs SET state = CASE WHEN started_at_ns IS NULL THEN 'failed' ELSE 'rollback_pending' END, error = ?, ended_at_ns = CASE WHEN started_at_ns IS NULL THEN ? ELSE ended_at_ns END WHERE id = ? AND state IN ('queued','validating','running','stopping');

-- tumult-lake/src/sql/runs.rs:FINISH
UPDATE runs SET state = ?, ended_at_ns = ?, experiment_id = COALESCE(?, experiment_id), rollback_status = ?, error = ? WHERE id = ? AND (NOT ? OR state = 'pending_approval');

-- tumult-lake/src/sql/runs.rs:LAST_AUDIT_HASH
SELECT new_hash FROM run_audit WHERE run_id = ? ORDER BY at_ns DESC, rowid DESC LIMIT 1;

-- tumult-lake/src/sql/runs.rs:INSERT_AUDIT
INSERT INTO run_audit (run_id, at_ns, event, detail, actor, prev_hash, new_hash) VALUES (?,?,?,?,?,?,?);

-- tumult-lake/src/sql/runs.rs:EXECUTION_HASH
SELECT execution_hash FROM run_execution_pins WHERE run_id = ?;

-- tumult-lake/src/sql/runs.rs:active_campaign("reg-1")
SELECT r.id FROM runs r WHERE r.registry_id = 'reg-1' AND r.gameday_id IS NULL AND r.state IN ('queued', 'validating', 'running', 'stopping', 'orphaned', 'rollback_pending') LIMIT 1;

-- tumult-lake/src/sql/runs.rs:definition("reg-1")
SELECT * FROM run_registry WHERE id = 'reg-1';

-- tumult-lake/src/sql/runs.rs:definition_by_hash("sha256:1")
SELECT * FROM run_registry WHERE content_hash = 'sha256:1' ORDER BY registered_at_ns LIMIT 1;

-- tumult-lake/src/sql/runs.rs:definitions(50)
SELECT id, name, content_hash, registered_at_ns, registered_by FROM run_registry ORDER BY registered_at_ns DESC LIMIT 50;

-- tumult-lake/src/sql/runs.rs:runs(Some("queued"), 50)
SELECT r.*, g.name AS definition_name FROM runs r LEFT JOIN run_registry g ON g.id = r.registry_id WHERE r.state = 'queued' ORDER BY r.queued_at_ns DESC LIMIT 50;

-- tumult-lake/src/sql/runs.rs:runs(None, 50)
SELECT r.*, g.name AS definition_name FROM runs r LEFT JOIN run_registry g ON g.id = r.registry_id  ORDER BY r.queued_at_ns DESC LIMIT 50;

-- tumult-lake/src/sql/runs.rs:run("run-1")
SELECT r.*, g.name AS definition_name FROM runs r LEFT JOIN run_registry g ON g.id = r.registry_id WHERE r.id = 'run-1';

-- tumult-lake/src/sql/runs.rs:audit_trail("run-1")
SELECT * FROM run_audit WHERE run_id = 'run-1' ORDER BY at_ns;

-- tumult-lake/src/sql/runs.rs:active_runs()
SELECT r.*, g.name AS definition_name, g.definition_toon, p.plan_json AS recovery_plan_json FROM runs r JOIN run_registry g ON g.id = r.registry_id LEFT JOIN run_recovery_plans p ON p.run_id = r.id WHERE r.state IN ('queued', 'validating', 'running', 'stopping', 'orphaned', 'rollback_pending') AND g.kind IS DISTINCT FROM 'gameday' ORDER BY r.queued_at_ns;

-- tumult-lake/src/sql/schedules.rs:INSERT
INSERT INTO run_schedules VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?);

-- tumult-lake/src/sql/schedules.rs:SET_ENABLED
UPDATE run_schedules SET enabled = ? WHERE id = ?;

-- tumult-lake/src/sql/schedules.rs:DELETE
DELETE FROM run_schedules WHERE id = ?;

-- tumult-lake/src/sql/schedules.rs:FIRED
UPDATE run_schedules SET last_run_at_ns = ?, last_run_id = ?, next_run_at_ns = ? WHERE id = ?;

-- tumult-lake/src/sql/schedules.rs:LIST
SELECT * FROM run_schedules ORDER BY name;

-- tumult-lake/src/sql/schedules.rs:due(1_774_980_000_000_000_000)
SELECT * FROM run_schedules WHERE enabled AND next_run_at_ns <= 1774980000000000000 ORDER BY next_run_at_ns;

-- tumult-lake/src/sql/sightings.rs:AHEAD_STORED
SELECT service_name, target_environment, service_version,
           count(*) AS spans, min(ts_ns) AS first_ns
    FROM spans
    WHERE ts_ns > ? AND ts_ns <= ?
      AND service_version IS NOT NULL AND service_version <> ''
    GROUP BY service_name, target_environment, service_version
    ORDER BY spans DESC, first_ns
    LIMIT 1;

-- tumult-lake/src/sql/sightings.rs:UPDATE
UPDATE service_version_sightings
    SET first_seen_ns = ?, last_seen_ns = ?, last_deployed_ns = ?,
        deployments = ?, sanitized = ?, first_anchor_ns = ?
    WHERE service_name = ? AND service_version = ?
      AND environment IS NOT DISTINCT FROM ?;

-- tumult-lake/src/sql/sightings.rs:INSERT
INSERT INTO service_version_sightings
       (service_name, environment, service_version, first_seen_ns, last_seen_ns,
        last_deployed_ns, deployments, sanitized, first_anchor_ns)
    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?);

-- tumult-lake/src/sql/sightings.rs:BACKFILL_STATE
SELECT done, cursor_ns FROM sightings_backfill LIMIT 1;

-- tumult-lake/src/sql/sightings.rs:BACKFILL_RESTART
DELETE FROM service_version_sightings;
    DELETE FROM sightings_backfill;
    INSERT INTO sightings_backfill (done, cursor_ns) VALUES (false, NULL);

-- tumult-lake/src/sql/sightings.rs:NEXT_VERSIONED_SPAN
SELECT min(ts_ns) FROM spans
    WHERE ts_ns > ? AND service_version IS NOT NULL AND service_version <> '';

-- tumult-lake/src/sql/sightings.rs:BACKFILL_DONE
UPDATE sightings_backfill SET done = true;

-- tumult-lake/src/sql/sightings.rs:BACKFILL_SLICE
SELECT service_name, target_environment, service_version,
           min(ts_ns) AS first_ns, max(ts_ns) AS last_ns
    FROM spans
    WHERE ts_ns > ? AND ts_ns <= ?
      AND service_version IS NOT NULL AND service_version <> ''
    GROUP BY service_name, target_environment, service_version, ts_ns // ?
    ORDER BY first_ns;

-- tumult-lake/src/sql/sightings.rs:SET_BACKFILL_CURSOR
UPDATE sightings_backfill SET cursor_ns = ?;

-- tumult-lake/src/sql/sightings.rs:OBSERVED_VERSIONS
SELECT service_version, count(*) FROM spans
    WHERE service_name = ? AND ts_ns BETWEEN ? AND ?
      AND target_environment IS NOT DISTINCT FROM ?
      AND service_version IS NOT NULL AND service_version <> ''
      AND (upper(span_kind) IN ('SERVER', 'CONSUMER')
           OR parent_span_id IS NULL OR parent_span_id = '')
      AND (? IS NULL OR resource_attrs['service.namespace'] = ?)
    GROUP BY service_version;

-- tumult-lake/src/sql/store.rs:SCHEMA_META_EXISTS
SELECT count(*) > 0 FROM information_schema.tables WHERE table_name = 'schema_meta';

-- tumult-lake/src/sql/store.rs:NEWEST_SCHEMA_VERSION
SELECT max(value) FROM schema_meta WHERE key = 'version';

-- tumult-lake/src/sql/store.rs:SCHEMA_VERSION
SELECT value FROM schema_meta WHERE key = 'version';

-- tumult-lake/src/sql/store.rs:SCHEMA_VERSION_AS_V
SELECT value AS v FROM schema_meta WHERE key = 'version';

-- tumult-lake/src/sql/store.rs:INSERT_LEGACY_USER
INSERT INTO users VALUES ('legacy', 'legacy', '!', 'viewer', false, true, 0);

-- tumult-lake/src/sql/store.rs:INSERT_SCHEMA_VERSION
INSERT INTO schema_meta (key, value) VALUES ('version', ?);

-- tumult-lake/src/sql/store.rs:UPDATE_SCHEMA_VERSION
UPDATE schema_meta SET value = ? WHERE key = 'version';

-- tumult-lake/src/sql/tx.rs:BEGIN
BEGIN TRANSACTION;

-- tumult-lake/src/sql/tx.rs:COMMIT
COMMIT;

-- tumult-lake/src/sql/tx.rs:ROLLBACK
ROLLBACK;

-- tumult-lake/src/sql/webhooks.rs:INSERT
INSERT INTO webhooks VALUES (?,?,?,?,?,?,?,?);

-- tumult-lake/src/sql/webhooks.rs:SET_ENABLED
UPDATE webhooks SET enabled = ? WHERE id = ?;

-- tumult-lake/src/sql/webhooks.rs:DELETE
DELETE FROM webhooks WHERE id = ?;

-- tumult-lake/src/sql/webhooks.rs:DELETE_CURSOR
DELETE FROM webhook_cursors WHERE webhook_id = ?;

-- tumult-lake/src/sql/webhooks.rs:INSERT_CURSOR
INSERT INTO webhook_cursors VALUES (?,?);

-- tumult-lake/src/sql/webhooks.rs:INSERT_DEAD_LETTER
INSERT INTO webhook_dead_letters VALUES (?,?,?,?,?,?,?,?,?);

-- tumult-lake/src/sql/webhooks.rs:LIST
SELECT * FROM webhooks ORDER BY name;

-- tumult-lake/src/sql/webhooks.rs:LIST_ENABLED
SELECT * FROM webhooks WHERE enabled ORDER BY name;

-- tumult-lake/src/sql/webhooks.rs:cursor("hook-1")
SELECT last_at_ns FROM webhook_cursors WHERE webhook_id = 'hook-1';

-- tumult-lake/src/sql/writer.rs:INSERT_SPAN
INSERT INTO spans VALUES (
    ?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
    CAST(? AS JSON));

-- tumult-lake/src/sql/writer.rs:INSERT_LOG
INSERT INTO logs (ts_ns, severity_text, body, trace_id, span_id,
       service_name, log_attrs, resource_attrs, event_name, scope_attrs)
    VALUES (?,?,?,?,?,?,
       CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
       CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
       ?,
       CAST(json(?) AS MAP(VARCHAR,VARCHAR)));

-- tumult-lake/src/sql/writer.rs:INSERT_METRIC_SUM
INSERT INTO metric_sums VALUES (?,?,?,?,?,?,
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)));

-- tumult-lake/src/sql/writer.rs:INSERT_METRIC_GAUGE
INSERT INTO metric_gauges VALUES (?,?,?,?,?,?,
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)));

-- tumult-lake/src/sql/writer.rs:INSERT_METRIC_HISTOGRAM
INSERT INTO metric_histograms VALUES (?,?,?,?,?,?,
    CAST(? AS BIGINT[]), CAST(? AS DOUBLE[]),
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
    ?,?,?);

-- tumult-lake/src/sql/writer.rs:INSERT_IMPORT_BATCH
INSERT INTO import_batches VALUES (?,?,?,?,?);

-- tumult-lake/src/backup.rs:snapshot_sql("lake.duckdb", "backup/store.duckdb")
-- Not prepared: it attaches two stores and copies one into the other.
-- Not linted: sqlfluff has no grammar for ATTACH, COPY FROM DATABASE and DETACH.
-- ATTACH 'lake.duckdb' AS source_store (READ_ONLY); ATTACH 'backup/store.duckdb' AS backup_store; COPY FROM DATABASE source_store TO backup_store; CHECKPOINT backup_store; DETACH backup_store; DETACH source_store;

-- tumult-lake/src/dr.rs:tests_sql()
SELECT * FROM (WITH
rto AS (
    SELECT
        experiment_id,
        bool_and(recovered) AS recovered
    FROM nfr_verdicts
    WHERE kind = 'rto' AND nfr_id = 'rto'
    GROUP BY experiment_id
),

roots AS (
    SELECT
        experiment_id,
        any_value(target_system) AS target_system,
        any_value(target_environment) AS target_environment
    FROM run_roots
    GROUP BY experiment_id
),

automated AS (
    SELECT
        'automated' AS origin,
        e.experiment_id AS evidence_id,
        e.title AS experiment_name,
        NULL AS exercise_type,
        r.target_system,
        r.target_environment,
        e.started_at_ns AS executed_at_ns,
        lower(e.status) AS outcome,
        e.rto_target_s,
        coalesce(e.rto_measured_s, e.mttr_s) AS rto_measured_s,
        coalesce(
            v.recovered,
            CASE WHEN e.mttr_s IS NOT NULL THEN TRUE END
        ) AS recovered,
        NOT e.impact_observed AS no_outage,
        e.rpo_target_s,
        e.rpo_measured_s,
        NULL AS data_verified,
        NULL AS reviewer,
        NULL AS plan_id
    FROM experiments AS e
    LEFT JOIN roots AS r ON e.experiment_id = r.experiment_id
    LEFT JOIN rto AS v ON e.experiment_id = v.experiment_id
    WHERE e.test_kind = 'dr'
),

manual AS (
    SELECT
        'manual' AS origin,
        m.id AS evidence_id,
        m.experiment_name,
        m.exercise_type,
        m.target_system,
        m.target_environment,
        m.executed_at_ns,
        m.outcome_status AS outcome,
        m.rto_target_s,
        m.recovery_time_s AS rto_measured_s,
        CASE WHEN m.recovery_time_s IS NOT NULL THEN TRUE END AS recovered,
        NULL AS no_outage,
        m.rpo_target_s,
        m.rpo_measured_s,
        m.data_verified,
        m.reviewed_by AS reviewer,
        m.plan_id
    FROM manual_experiments AS m
    WHERE
        m.status IN ('submitted', 'verified')
        AND list_contains(from_json(CAST($1 AS VARCHAR), '["VARCHAR"]'), m.exercise_type)
),

tests AS (
    SELECT * FROM automated
    UNION ALL
    SELECT * FROM manual
),

judged AS (
    SELECT
        *,
        origin || ':' || evidence_id AS test_id,
        CASE
            WHEN NOT recovered THEN 'not_recovered'
            WHEN
                rto_measured_s > rto_target_s
                OR rpo_measured_s > rpo_target_s
                THEN 'breached'
            WHEN
                coalesce(rto_target_s, rpo_target_s) IS NOT NULL
                AND (rto_target_s IS NULL OR rto_measured_s IS NOT NULL)
                AND (rpo_target_s IS NULL OR rpo_measured_s IS NOT NULL)
                THEN 'met'
            ELSE 'not_measured'
        END AS verdict
    FROM tests
)

SELECT
    test_id,
    origin,
    experiment_name,
    exercise_type,
    target_system,
    target_environment,
    executed_at_ns,
    outcome,
    rto_target_s,
    rto_measured_s,
    recovered,
    no_outage,
    rpo_target_s,
    rpo_measured_s,
    data_verified,
    verdict,
    evidence_id,
    reviewer,
    plan_id
FROM judged) AS t WHERE $2 IS NULL OR list_contains(from_json(CAST($2 AS VARCHAR), '["VARCHAR"]'), t.target_environment) ORDER BY t.executed_at_ns, t.test_id;

-- tumult-lake/src/dr.rs:due_register_sql()
SELECT * FROM (WITH
dr_tests AS (
    WITH
    rto AS (
        SELECT
            experiment_id,
            bool_and(recovered) AS recovered
        FROM nfr_verdicts
        WHERE kind = 'rto' AND nfr_id = 'rto'
        GROUP BY experiment_id
    ),

    roots AS (
        SELECT
            experiment_id,
            any_value(target_system) AS target_system,
            any_value(target_environment) AS target_environment
        FROM run_roots
        GROUP BY experiment_id
    ),

    automated AS (
        SELECT
            'automated' AS origin,
            e.experiment_id AS evidence_id,
            e.title AS experiment_name,
            NULL AS exercise_type,
            r.target_system,
            r.target_environment,
            e.started_at_ns AS executed_at_ns,
            lower(e.status) AS outcome,
            e.rto_target_s,
            coalesce(e.rto_measured_s, e.mttr_s) AS rto_measured_s,
            coalesce(
                v.recovered,
                CASE WHEN e.mttr_s IS NOT NULL THEN TRUE END
            ) AS recovered,
            NOT e.impact_observed AS no_outage,
            e.rpo_target_s,
            e.rpo_measured_s,
            NULL AS data_verified,
            NULL AS reviewer,
            NULL AS plan_id
        FROM experiments AS e
        LEFT JOIN roots AS r ON e.experiment_id = r.experiment_id
        LEFT JOIN rto AS v ON e.experiment_id = v.experiment_id
        WHERE e.test_kind = 'dr'
    ),

    manual AS (
        SELECT
            'manual' AS origin,
            m.id AS evidence_id,
            m.experiment_name,
            m.exercise_type,
            m.target_system,
            m.target_environment,
            m.executed_at_ns,
            m.outcome_status AS outcome,
            m.rto_target_s,
            m.recovery_time_s AS rto_measured_s,
            CASE WHEN m.recovery_time_s IS NOT NULL THEN TRUE END AS recovered,
            NULL AS no_outage,
            m.rpo_target_s,
            m.rpo_measured_s,
            m.data_verified,
            m.reviewed_by AS reviewer,
            m.plan_id
        FROM manual_experiments AS m
        WHERE
            m.status IN ('submitted', 'verified')
            AND list_contains(from_json(CAST($1 AS VARCHAR), '["VARCHAR"]'), m.exercise_type)
    ),

    tests AS (
        SELECT * FROM automated
        UNION ALL
        SELECT * FROM manual
    ),

    judged AS (
        SELECT
            *,
            origin || ':' || evidence_id AS test_id,
            CASE
                WHEN NOT recovered THEN 'not_recovered'
                WHEN
                    rto_measured_s > rto_target_s
                    OR rpo_measured_s > rpo_target_s
                    THEN 'breached'
                WHEN
                    coalesce(rto_target_s, rpo_target_s) IS NOT NULL
                    AND (rto_target_s IS NULL OR rto_measured_s IS NOT NULL)
                    AND (rpo_target_s IS NULL OR rpo_measured_s IS NOT NULL)
                    THEN 'met'
                ELSE 'not_measured'
            END AS verdict
        FROM tests
    )

    SELECT
        test_id,
        origin,
        experiment_name,
        exercise_type,
        target_system,
        target_environment,
        executed_at_ns,
        outcome,
        rto_target_s,
        rto_measured_s,
        recovered,
        no_outage,
        rpo_target_s,
        rpo_measured_s,
        data_verified,
        verdict,
        evidence_id,
        reviewer,
        plan_id
    FROM judged
),

counted AS (
    SELECT
        target_system,
        target_environment,
        test_id,
        executed_at_ns
    FROM dr_tests
    WHERE
        target_system IS NOT NULL
        AND executed_at_ns <= $2
        AND outcome NOT IN ('aborted', 'interrupted', 'inconclusive')
        AND NOT coalesce(no_outage, FALSE)
),

last_tests AS (
    SELECT
        target_system,
        target_environment,
        max(executed_at_ns) AS last_test_at_ns,
        arg_max(test_id, executed_at_ns) AS last_test_id
    FROM counted
    GROUP BY target_system, target_environment
),

dr_plans AS (
    SELECT
        id,
        target_system,
        target_environment,
        interval_days,
        due_at_ns
    FROM manual_exercise_plans
    WHERE
        status = 'planned'
        AND target_system IS NOT NULL
        AND list_contains(from_json(CAST($1 AS VARCHAR), '["VARCHAR"]'), exercise_type)
),

declared AS (
    SELECT
        r.target_system,
        r.target_environment
    FROM experiments AS e
    INNER JOIN run_roots AS r ON e.experiment_id = r.experiment_id
    WHERE e.rto_target_s IS NOT NULL
    UNION
    SELECT
        target_system,
        target_environment
    FROM manual_experiments
    WHERE rto_target_s IS NOT NULL AND status IN ('submitted', 'verified')
    UNION
    SELECT
        target_system,
        target_environment
    FROM manual_exercise_plans
    WHERE rto_target_s IS NOT NULL AND status = 'planned'
),

seen AS (
    SELECT
        target_system,
        target_environment
    FROM dr_tests
    UNION
    SELECT
        target_system,
        target_environment
    FROM declared
),

targets AS (
    SELECT
        target_system,
        target_environment
    FROM seen
    WHERE target_system IS NOT NULL
    UNION
    SELECT
        p.target_system,
        p.target_environment
    FROM dr_plans AS p
    WHERE NOT EXISTS (
        SELECT 1 FROM seen AS s
        WHERE
            s.target_system = p.target_system
            AND (
                p.target_environment IS NULL
                OR s.target_environment = p.target_environment
            )
    )
),

covering_plans AS (
    SELECT
        t.target_system,
        t.target_environment,
        arg_min(p.id, p.due_at_ns) AS plan_id,
        min(p.due_at_ns) AS plan_due_at_ns,
        min(p.interval_days) AS interval_days
    FROM targets AS t
    INNER JOIN dr_plans AS p
        ON
            t.target_system = p.target_system
            AND (
                p.target_environment IS NULL
                OR t.target_environment = p.target_environment
            )
    GROUP BY t.target_system, t.target_environment
),

planned_tests AS (
    SELECT
        function_id,
        unnest(
            from_json(test_plan, '[{"kind": "VARCHAR", "interval_days": "BIGINT"}]')
        ) AS planned
    FROM ict_functions
    WHERE active
),

function_intervals AS (
    SELECT
        function_id,
        min(
            least(struct_extract(planned, 'interval_days'), 3650)
        ) AS interval_days
    FROM planned_tests
    WHERE
        struct_extract(planned, 'kind') = 'dr'
        AND struct_extract(planned, 'interval_days') > 0
    GROUP BY function_id
),

target_names AS (
    SELECT DISTINCT target_system FROM targets
),

service_patterns AS (
    SELECT
        function_id,
        kind,
        pattern,
        replace(
            replace(replace(replace(pattern, '!', '!!'), '%', '!%'), '_', '!_'),
            '*',
            '%'
        ) AS like_pattern
    FROM ict_function_services
),

bound_functions AS (
    SELECT
        t.target_system,
        f.function_id
    FROM target_names AS t
    INNER JOIN function_intervals AS f ON t.target_system = f.function_id
    UNION
    SELECT
        t.target_system,
        s.function_id
    FROM target_names AS t
    INNER JOIN service_patterns AS s
        ON
            (s.kind = 'entity' AND t.target_system = s.pattern)
            OR (
                s.kind = 'service'
                AND t.target_system LIKE s.like_pattern ESCAPE '!'
            )
),

target_intervals AS (
    SELECT
        b.target_system,
        min(f.interval_days) AS interval_days
    FROM bound_functions AS b
    INNER JOIN function_intervals AS f ON b.function_id = f.function_id
    GROUP BY b.target_system
),

due_rows AS (
    SELECT
        t.target_system,
        t.target_environment,
        l.last_test_at_ns,
        l.last_test_id,
        c.plan_id,
        c.plan_due_at_ns,
        coalesce(c.interval_days, i.interval_days, 365) AS interval_days,
        CASE
            WHEN c.interval_days IS NOT NULL THEN 'plan'
            WHEN i.interval_days IS NOT NULL THEN 'function'
            ELSE 'default'
        END AS interval_source
    FROM targets AS t
    LEFT JOIN last_tests AS l
        ON
            t.target_system = l.target_system
            AND t.target_environment IS NOT DISTINCT FROM l.target_environment
    LEFT JOIN covering_plans AS c
        ON
            t.target_system = c.target_system
            AND t.target_environment IS NOT DISTINCT FROM c.target_environment
    LEFT JOIN target_intervals AS i ON t.target_system = i.target_system
)

SELECT
    target_system,
    target_environment,
    last_test_at_ns,
    last_test_id,
    interval_days,
    interval_source,
    plan_id,
    plan_due_at_ns,
    last_test_at_ns + interval_days * 86400000000000 AS next_due_at_ns,
    CASE
        WHEN last_test_at_ns IS NULL THEN 'never_tested'
        WHEN last_test_at_ns + interval_days * 86400000000000 < $2 THEN 'overdue'
        WHEN
            last_test_at_ns + interval_days * 86400000000000 - $2
            <= 2592000000000000
            THEN 'due'
        ELSE 'ok'
    END AS state
FROM due_rows) AS r WHERE $3 IS NULL OR list_contains(from_json(CAST($3 AS VARCHAR), '["VARCHAR"]'), r.target_environment) ORDER BY r.target_system, r.target_environment NULLS FIRST;

-- tumult-lake/src/dr.rs:kept_tests_sql()
SELECT * FROM (WITH
rto AS (
    SELECT
        experiment_id,
        bool_and(recovered) AS recovered
    FROM nfr_verdicts
    WHERE kind = 'rto' AND nfr_id = 'rto'
    GROUP BY experiment_id
),

roots AS (
    SELECT
        experiment_id,
        any_value(target_system) AS target_system,
        any_value(target_environment) AS target_environment
    FROM run_roots
    GROUP BY experiment_id
),

automated AS (
    SELECT
        'automated' AS origin,
        e.experiment_id AS evidence_id,
        e.title AS experiment_name,
        NULL AS exercise_type,
        r.target_system,
        r.target_environment,
        e.started_at_ns AS executed_at_ns,
        lower(e.status) AS outcome,
        e.rto_target_s,
        coalesce(e.rto_measured_s, e.mttr_s) AS rto_measured_s,
        coalesce(
            v.recovered,
            CASE WHEN e.mttr_s IS NOT NULL THEN TRUE END
        ) AS recovered,
        NOT e.impact_observed AS no_outage,
        e.rpo_target_s,
        e.rpo_measured_s,
        NULL AS data_verified,
        NULL AS reviewer,
        NULL AS plan_id
    FROM experiments AS e
    LEFT JOIN roots AS r ON e.experiment_id = r.experiment_id
    LEFT JOIN rto AS v ON e.experiment_id = v.experiment_id
    WHERE e.test_kind = 'dr'
),

manual AS (
    SELECT
        'manual' AS origin,
        m.id AS evidence_id,
        m.experiment_name,
        m.exercise_type,
        m.target_system,
        m.target_environment,
        m.executed_at_ns,
        m.outcome_status AS outcome,
        m.rto_target_s,
        m.recovery_time_s AS rto_measured_s,
        CASE WHEN m.recovery_time_s IS NOT NULL THEN TRUE END AS recovered,
        NULL AS no_outage,
        m.rpo_target_s,
        m.rpo_measured_s,
        m.data_verified,
        m.reviewed_by AS reviewer,
        m.plan_id
    FROM manual_experiments AS m
    WHERE
        m.status IN ('submitted', 'verified')
        AND list_contains(from_json(CAST($1 AS VARCHAR), '["VARCHAR"]'), m.exercise_type)
),

tests AS (
    SELECT * FROM automated
    UNION ALL
    SELECT * FROM manual
),

judged AS (
    SELECT
        *,
        origin || ':' || evidence_id AS test_id,
        CASE
            WHEN NOT recovered THEN 'not_recovered'
            WHEN
                rto_measured_s > rto_target_s
                OR rpo_measured_s > rpo_target_s
                THEN 'breached'
            WHEN
                coalesce(rto_target_s, rpo_target_s) IS NOT NULL
                AND (rto_target_s IS NULL OR rto_measured_s IS NOT NULL)
                AND (rpo_target_s IS NULL OR rpo_measured_s IS NOT NULL)
                THEN 'met'
            ELSE 'not_measured'
        END AS verdict
    FROM tests
)

SELECT
    test_id,
    origin,
    experiment_name,
    exercise_type,
    target_system,
    target_environment,
    executed_at_ns,
    outcome,
    rto_target_s,
    rto_measured_s,
    recovered,
    no_outage,
    rpo_target_s,
    rpo_measured_s,
    data_verified,
    verdict,
    evidence_id,
    reviewer,
    plan_id
FROM judged) AS t WHERE ($2 IS NULL OR list_contains(from_json(CAST($2 AS VARCHAR), '["VARCHAR"]'), t.target_environment)) AND (CAST($3 AS VARCHAR) IS NULL OR t.target_system = CAST($3 AS VARCHAR)) AND (CAST($4 AS BIGINT) IS NULL OR t.executed_at_ns >= CAST($4 AS BIGINT)) AND (CAST($5 AS BIGINT) IS NULL OR t.executed_at_ns <= CAST($5 AS BIGINT));

-- tumult-lake/src/dr.rs:tests_page_sql(50)
SELECT * FROM (WITH
rto AS (
    SELECT
        experiment_id,
        bool_and(recovered) AS recovered
    FROM nfr_verdicts
    WHERE kind = 'rto' AND nfr_id = 'rto'
    GROUP BY experiment_id
),

roots AS (
    SELECT
        experiment_id,
        any_value(target_system) AS target_system,
        any_value(target_environment) AS target_environment
    FROM run_roots
    GROUP BY experiment_id
),

automated AS (
    SELECT
        'automated' AS origin,
        e.experiment_id AS evidence_id,
        e.title AS experiment_name,
        NULL AS exercise_type,
        r.target_system,
        r.target_environment,
        e.started_at_ns AS executed_at_ns,
        lower(e.status) AS outcome,
        e.rto_target_s,
        coalesce(e.rto_measured_s, e.mttr_s) AS rto_measured_s,
        coalesce(
            v.recovered,
            CASE WHEN e.mttr_s IS NOT NULL THEN TRUE END
        ) AS recovered,
        NOT e.impact_observed AS no_outage,
        e.rpo_target_s,
        e.rpo_measured_s,
        NULL AS data_verified,
        NULL AS reviewer,
        NULL AS plan_id
    FROM experiments AS e
    LEFT JOIN roots AS r ON e.experiment_id = r.experiment_id
    LEFT JOIN rto AS v ON e.experiment_id = v.experiment_id
    WHERE e.test_kind = 'dr'
),

manual AS (
    SELECT
        'manual' AS origin,
        m.id AS evidence_id,
        m.experiment_name,
        m.exercise_type,
        m.target_system,
        m.target_environment,
        m.executed_at_ns,
        m.outcome_status AS outcome,
        m.rto_target_s,
        m.recovery_time_s AS rto_measured_s,
        CASE WHEN m.recovery_time_s IS NOT NULL THEN TRUE END AS recovered,
        NULL AS no_outage,
        m.rpo_target_s,
        m.rpo_measured_s,
        m.data_verified,
        m.reviewed_by AS reviewer,
        m.plan_id
    FROM manual_experiments AS m
    WHERE
        m.status IN ('submitted', 'verified')
        AND list_contains(from_json(CAST($1 AS VARCHAR), '["VARCHAR"]'), m.exercise_type)
),

tests AS (
    SELECT * FROM automated
    UNION ALL
    SELECT * FROM manual
),

judged AS (
    SELECT
        *,
        origin || ':' || evidence_id AS test_id,
        CASE
            WHEN NOT recovered THEN 'not_recovered'
            WHEN
                rto_measured_s > rto_target_s
                OR rpo_measured_s > rpo_target_s
                THEN 'breached'
            WHEN
                coalesce(rto_target_s, rpo_target_s) IS NOT NULL
                AND (rto_target_s IS NULL OR rto_measured_s IS NOT NULL)
                AND (rpo_target_s IS NULL OR rpo_measured_s IS NOT NULL)
                THEN 'met'
            ELSE 'not_measured'
        END AS verdict
    FROM tests
)

SELECT
    test_id,
    origin,
    experiment_name,
    exercise_type,
    target_system,
    target_environment,
    executed_at_ns,
    outcome,
    rto_target_s,
    rto_measured_s,
    recovered,
    no_outage,
    rpo_target_s,
    rpo_measured_s,
    data_verified,
    verdict,
    evidence_id,
    reviewer,
    plan_id
FROM judged) AS t WHERE ($2 IS NULL OR list_contains(from_json(CAST($2 AS VARCHAR), '["VARCHAR"]'), t.target_environment)) AND (CAST($3 AS VARCHAR) IS NULL OR t.target_system = CAST($3 AS VARCHAR)) AND (CAST($4 AS BIGINT) IS NULL OR t.executed_at_ns >= CAST($4 AS BIGINT)) AND (CAST($5 AS BIGINT) IS NULL OR t.executed_at_ns <= CAST($5 AS BIGINT)) AND (CAST($6 AS BIGINT) IS NULL OR t.executed_at_ns < CAST($6 AS BIGINT) OR (t.executed_at_ns = CAST($6 AS BIGINT) AND t.test_id > CAST($7 AS VARCHAR))) ORDER BY t.executed_at_ns DESC, t.test_id LIMIT 50;

-- tumult-lake/src/dr.rs:test_counts_sql()
SELECT k.origin, k.verdict, count(*) AS tests FROM (SELECT * FROM (WITH
rto AS (
    SELECT
        experiment_id,
        bool_and(recovered) AS recovered
    FROM nfr_verdicts
    WHERE kind = 'rto' AND nfr_id = 'rto'
    GROUP BY experiment_id
),

roots AS (
    SELECT
        experiment_id,
        any_value(target_system) AS target_system,
        any_value(target_environment) AS target_environment
    FROM run_roots
    GROUP BY experiment_id
),

automated AS (
    SELECT
        'automated' AS origin,
        e.experiment_id AS evidence_id,
        e.title AS experiment_name,
        NULL AS exercise_type,
        r.target_system,
        r.target_environment,
        e.started_at_ns AS executed_at_ns,
        lower(e.status) AS outcome,
        e.rto_target_s,
        coalesce(e.rto_measured_s, e.mttr_s) AS rto_measured_s,
        coalesce(
            v.recovered,
            CASE WHEN e.mttr_s IS NOT NULL THEN TRUE END
        ) AS recovered,
        NOT e.impact_observed AS no_outage,
        e.rpo_target_s,
        e.rpo_measured_s,
        NULL AS data_verified,
        NULL AS reviewer,
        NULL AS plan_id
    FROM experiments AS e
    LEFT JOIN roots AS r ON e.experiment_id = r.experiment_id
    LEFT JOIN rto AS v ON e.experiment_id = v.experiment_id
    WHERE e.test_kind = 'dr'
),

manual AS (
    SELECT
        'manual' AS origin,
        m.id AS evidence_id,
        m.experiment_name,
        m.exercise_type,
        m.target_system,
        m.target_environment,
        m.executed_at_ns,
        m.outcome_status AS outcome,
        m.rto_target_s,
        m.recovery_time_s AS rto_measured_s,
        CASE WHEN m.recovery_time_s IS NOT NULL THEN TRUE END AS recovered,
        NULL AS no_outage,
        m.rpo_target_s,
        m.rpo_measured_s,
        m.data_verified,
        m.reviewed_by AS reviewer,
        m.plan_id
    FROM manual_experiments AS m
    WHERE
        m.status IN ('submitted', 'verified')
        AND list_contains(from_json(CAST($1 AS VARCHAR), '["VARCHAR"]'), m.exercise_type)
),

tests AS (
    SELECT * FROM automated
    UNION ALL
    SELECT * FROM manual
),

judged AS (
    SELECT
        *,
        origin || ':' || evidence_id AS test_id,
        CASE
            WHEN NOT recovered THEN 'not_recovered'
            WHEN
                rto_measured_s > rto_target_s
                OR rpo_measured_s > rpo_target_s
                THEN 'breached'
            WHEN
                coalesce(rto_target_s, rpo_target_s) IS NOT NULL
                AND (rto_target_s IS NULL OR rto_measured_s IS NOT NULL)
                AND (rpo_target_s IS NULL OR rpo_measured_s IS NOT NULL)
                THEN 'met'
            ELSE 'not_measured'
        END AS verdict
    FROM tests
)

SELECT
    test_id,
    origin,
    experiment_name,
    exercise_type,
    target_system,
    target_environment,
    executed_at_ns,
    outcome,
    rto_target_s,
    rto_measured_s,
    recovered,
    no_outage,
    rpo_target_s,
    rpo_measured_s,
    data_verified,
    verdict,
    evidence_id,
    reviewer,
    plan_id
FROM judged) AS t WHERE ($2 IS NULL OR list_contains(from_json(CAST($2 AS VARCHAR), '["VARCHAR"]'), t.target_environment)) AND (CAST($3 AS VARCHAR) IS NULL OR t.target_system = CAST($3 AS VARCHAR)) AND (CAST($4 AS BIGINT) IS NULL OR t.executed_at_ns >= CAST($4 AS BIGINT)) AND (CAST($5 AS BIGINT) IS NULL OR t.executed_at_ns <= CAST($5 AS BIGINT))) AS k GROUP BY k.origin, k.verdict ORDER BY k.origin, k.verdict;

-- tumult-lake/src/dr.rs:targets_sql()
SELECT r.target_system, r.target_environment, r.state, r.last_test_id, r.last_test_at_ns, t.origin AS last_test_origin, t.verdict AS last_test_verdict, r.next_due_at_ns, r.interval_days, r.interval_source, r.plan_id, r.plan_due_at_ns, coalesce(t.rto_target_s, p.rto_target_s) AS rto_target_s, coalesce(t.rpo_target_s, p.rpo_target_s) AS rpo_target_s, t.rto_measured_s, t.rpo_measured_s, t.recovered FROM (WITH
dr_tests AS (
    WITH
    rto AS (
        SELECT
            experiment_id,
            bool_and(recovered) AS recovered
        FROM nfr_verdicts
        WHERE kind = 'rto' AND nfr_id = 'rto'
        GROUP BY experiment_id
    ),

    roots AS (
        SELECT
            experiment_id,
            any_value(target_system) AS target_system,
            any_value(target_environment) AS target_environment
        FROM run_roots
        GROUP BY experiment_id
    ),

    automated AS (
        SELECT
            'automated' AS origin,
            e.experiment_id AS evidence_id,
            e.title AS experiment_name,
            NULL AS exercise_type,
            r.target_system,
            r.target_environment,
            e.started_at_ns AS executed_at_ns,
            lower(e.status) AS outcome,
            e.rto_target_s,
            coalesce(e.rto_measured_s, e.mttr_s) AS rto_measured_s,
            coalesce(
                v.recovered,
                CASE WHEN e.mttr_s IS NOT NULL THEN TRUE END
            ) AS recovered,
            NOT e.impact_observed AS no_outage,
            e.rpo_target_s,
            e.rpo_measured_s,
            NULL AS data_verified,
            NULL AS reviewer,
            NULL AS plan_id
        FROM experiments AS e
        LEFT JOIN roots AS r ON e.experiment_id = r.experiment_id
        LEFT JOIN rto AS v ON e.experiment_id = v.experiment_id
        WHERE e.test_kind = 'dr'
    ),

    manual AS (
        SELECT
            'manual' AS origin,
            m.id AS evidence_id,
            m.experiment_name,
            m.exercise_type,
            m.target_system,
            m.target_environment,
            m.executed_at_ns,
            m.outcome_status AS outcome,
            m.rto_target_s,
            m.recovery_time_s AS rto_measured_s,
            CASE WHEN m.recovery_time_s IS NOT NULL THEN TRUE END AS recovered,
            NULL AS no_outage,
            m.rpo_target_s,
            m.rpo_measured_s,
            m.data_verified,
            m.reviewed_by AS reviewer,
            m.plan_id
        FROM manual_experiments AS m
        WHERE
            m.status IN ('submitted', 'verified')
            AND list_contains(from_json(CAST($1 AS VARCHAR), '["VARCHAR"]'), m.exercise_type)
    ),

    tests AS (
        SELECT * FROM automated
        UNION ALL
        SELECT * FROM manual
    ),

    judged AS (
        SELECT
            *,
            origin || ':' || evidence_id AS test_id,
            CASE
                WHEN NOT recovered THEN 'not_recovered'
                WHEN
                    rto_measured_s > rto_target_s
                    OR rpo_measured_s > rpo_target_s
                    THEN 'breached'
                WHEN
                    coalesce(rto_target_s, rpo_target_s) IS NOT NULL
                    AND (rto_target_s IS NULL OR rto_measured_s IS NOT NULL)
                    AND (rpo_target_s IS NULL OR rpo_measured_s IS NOT NULL)
                    THEN 'met'
                ELSE 'not_measured'
            END AS verdict
        FROM tests
    )

    SELECT
        test_id,
        origin,
        experiment_name,
        exercise_type,
        target_system,
        target_environment,
        executed_at_ns,
        outcome,
        rto_target_s,
        rto_measured_s,
        recovered,
        no_outage,
        rpo_target_s,
        rpo_measured_s,
        data_verified,
        verdict,
        evidence_id,
        reviewer,
        plan_id
    FROM judged
),

counted AS (
    SELECT
        target_system,
        target_environment,
        test_id,
        executed_at_ns
    FROM dr_tests
    WHERE
        target_system IS NOT NULL
        AND executed_at_ns <= $2
        AND outcome NOT IN ('aborted', 'interrupted', 'inconclusive')
        AND NOT coalesce(no_outage, FALSE)
),

last_tests AS (
    SELECT
        target_system,
        target_environment,
        max(executed_at_ns) AS last_test_at_ns,
        arg_max(test_id, executed_at_ns) AS last_test_id
    FROM counted
    GROUP BY target_system, target_environment
),

dr_plans AS (
    SELECT
        id,
        target_system,
        target_environment,
        interval_days,
        due_at_ns
    FROM manual_exercise_plans
    WHERE
        status = 'planned'
        AND target_system IS NOT NULL
        AND list_contains(from_json(CAST($1 AS VARCHAR), '["VARCHAR"]'), exercise_type)
),

declared AS (
    SELECT
        r.target_system,
        r.target_environment
    FROM experiments AS e
    INNER JOIN run_roots AS r ON e.experiment_id = r.experiment_id
    WHERE e.rto_target_s IS NOT NULL
    UNION
    SELECT
        target_system,
        target_environment
    FROM manual_experiments
    WHERE rto_target_s IS NOT NULL AND status IN ('submitted', 'verified')
    UNION
    SELECT
        target_system,
        target_environment
    FROM manual_exercise_plans
    WHERE rto_target_s IS NOT NULL AND status = 'planned'
),

seen AS (
    SELECT
        target_system,
        target_environment
    FROM dr_tests
    UNION
    SELECT
        target_system,
        target_environment
    FROM declared
),

targets AS (
    SELECT
        target_system,
        target_environment
    FROM seen
    WHERE target_system IS NOT NULL
    UNION
    SELECT
        p.target_system,
        p.target_environment
    FROM dr_plans AS p
    WHERE NOT EXISTS (
        SELECT 1 FROM seen AS s
        WHERE
            s.target_system = p.target_system
            AND (
                p.target_environment IS NULL
                OR s.target_environment = p.target_environment
            )
    )
),

covering_plans AS (
    SELECT
        t.target_system,
        t.target_environment,
        arg_min(p.id, p.due_at_ns) AS plan_id,
        min(p.due_at_ns) AS plan_due_at_ns,
        min(p.interval_days) AS interval_days
    FROM targets AS t
    INNER JOIN dr_plans AS p
        ON
            t.target_system = p.target_system
            AND (
                p.target_environment IS NULL
                OR t.target_environment = p.target_environment
            )
    GROUP BY t.target_system, t.target_environment
),

planned_tests AS (
    SELECT
        function_id,
        unnest(
            from_json(test_plan, '[{"kind": "VARCHAR", "interval_days": "BIGINT"}]')
        ) AS planned
    FROM ict_functions
    WHERE active
),

function_intervals AS (
    SELECT
        function_id,
        min(
            least(struct_extract(planned, 'interval_days'), 3650)
        ) AS interval_days
    FROM planned_tests
    WHERE
        struct_extract(planned, 'kind') = 'dr'
        AND struct_extract(planned, 'interval_days') > 0
    GROUP BY function_id
),

target_names AS (
    SELECT DISTINCT target_system FROM targets
),

service_patterns AS (
    SELECT
        function_id,
        kind,
        pattern,
        replace(
            replace(replace(replace(pattern, '!', '!!'), '%', '!%'), '_', '!_'),
            '*',
            '%'
        ) AS like_pattern
    FROM ict_function_services
),

bound_functions AS (
    SELECT
        t.target_system,
        f.function_id
    FROM target_names AS t
    INNER JOIN function_intervals AS f ON t.target_system = f.function_id
    UNION
    SELECT
        t.target_system,
        s.function_id
    FROM target_names AS t
    INNER JOIN service_patterns AS s
        ON
            (s.kind = 'entity' AND t.target_system = s.pattern)
            OR (
                s.kind = 'service'
                AND t.target_system LIKE s.like_pattern ESCAPE '!'
            )
),

target_intervals AS (
    SELECT
        b.target_system,
        min(f.interval_days) AS interval_days
    FROM bound_functions AS b
    INNER JOIN function_intervals AS f ON b.function_id = f.function_id
    GROUP BY b.target_system
),

due_rows AS (
    SELECT
        t.target_system,
        t.target_environment,
        l.last_test_at_ns,
        l.last_test_id,
        c.plan_id,
        c.plan_due_at_ns,
        coalesce(c.interval_days, i.interval_days, 365) AS interval_days,
        CASE
            WHEN c.interval_days IS NOT NULL THEN 'plan'
            WHEN i.interval_days IS NOT NULL THEN 'function'
            ELSE 'default'
        END AS interval_source
    FROM targets AS t
    LEFT JOIN last_tests AS l
        ON
            t.target_system = l.target_system
            AND t.target_environment IS NOT DISTINCT FROM l.target_environment
    LEFT JOIN covering_plans AS c
        ON
            t.target_system = c.target_system
            AND t.target_environment IS NOT DISTINCT FROM c.target_environment
    LEFT JOIN target_intervals AS i ON t.target_system = i.target_system
)

SELECT
    target_system,
    target_environment,
    last_test_at_ns,
    last_test_id,
    interval_days,
    interval_source,
    plan_id,
    plan_due_at_ns,
    last_test_at_ns + interval_days * 86400000000000 AS next_due_at_ns,
    CASE
        WHEN last_test_at_ns IS NULL THEN 'never_tested'
        WHEN last_test_at_ns + interval_days * 86400000000000 < $2 THEN 'overdue'
        WHEN
            last_test_at_ns + interval_days * 86400000000000 - $2
            <= 2592000000000000
            THEN 'due'
        ELSE 'ok'
    END AS state
FROM due_rows) AS r LEFT JOIN (WITH
rto AS (
    SELECT
        experiment_id,
        bool_and(recovered) AS recovered
    FROM nfr_verdicts
    WHERE kind = 'rto' AND nfr_id = 'rto'
    GROUP BY experiment_id
),

roots AS (
    SELECT
        experiment_id,
        any_value(target_system) AS target_system,
        any_value(target_environment) AS target_environment
    FROM run_roots
    GROUP BY experiment_id
),

automated AS (
    SELECT
        'automated' AS origin,
        e.experiment_id AS evidence_id,
        e.title AS experiment_name,
        NULL AS exercise_type,
        r.target_system,
        r.target_environment,
        e.started_at_ns AS executed_at_ns,
        lower(e.status) AS outcome,
        e.rto_target_s,
        coalesce(e.rto_measured_s, e.mttr_s) AS rto_measured_s,
        coalesce(
            v.recovered,
            CASE WHEN e.mttr_s IS NOT NULL THEN TRUE END
        ) AS recovered,
        NOT e.impact_observed AS no_outage,
        e.rpo_target_s,
        e.rpo_measured_s,
        NULL AS data_verified,
        NULL AS reviewer,
        NULL AS plan_id
    FROM experiments AS e
    LEFT JOIN roots AS r ON e.experiment_id = r.experiment_id
    LEFT JOIN rto AS v ON e.experiment_id = v.experiment_id
    WHERE e.test_kind = 'dr'
),

manual AS (
    SELECT
        'manual' AS origin,
        m.id AS evidence_id,
        m.experiment_name,
        m.exercise_type,
        m.target_system,
        m.target_environment,
        m.executed_at_ns,
        m.outcome_status AS outcome,
        m.rto_target_s,
        m.recovery_time_s AS rto_measured_s,
        CASE WHEN m.recovery_time_s IS NOT NULL THEN TRUE END AS recovered,
        NULL AS no_outage,
        m.rpo_target_s,
        m.rpo_measured_s,
        m.data_verified,
        m.reviewed_by AS reviewer,
        m.plan_id
    FROM manual_experiments AS m
    WHERE
        m.status IN ('submitted', 'verified')
        AND list_contains(from_json(CAST($1 AS VARCHAR), '["VARCHAR"]'), m.exercise_type)
),

tests AS (
    SELECT * FROM automated
    UNION ALL
    SELECT * FROM manual
),

judged AS (
    SELECT
        *,
        origin || ':' || evidence_id AS test_id,
        CASE
            WHEN NOT recovered THEN 'not_recovered'
            WHEN
                rto_measured_s > rto_target_s
                OR rpo_measured_s > rpo_target_s
                THEN 'breached'
            WHEN
                coalesce(rto_target_s, rpo_target_s) IS NOT NULL
                AND (rto_target_s IS NULL OR rto_measured_s IS NOT NULL)
                AND (rpo_target_s IS NULL OR rpo_measured_s IS NOT NULL)
                THEN 'met'
            ELSE 'not_measured'
        END AS verdict
    FROM tests
)

SELECT
    test_id,
    origin,
    experiment_name,
    exercise_type,
    target_system,
    target_environment,
    executed_at_ns,
    outcome,
    rto_target_s,
    rto_measured_s,
    recovered,
    no_outage,
    rpo_target_s,
    rpo_measured_s,
    data_verified,
    verdict,
    evidence_id,
    reviewer,
    plan_id
FROM judged) AS t ON r.last_test_id = t.test_id LEFT JOIN manual_exercise_plans AS p ON r.plan_id = p.id WHERE ($3 IS NULL OR list_contains(from_json(CAST($3 AS VARCHAR), '["VARCHAR"]'), r.target_environment)) AND (CAST($4 AS VARCHAR) IS NULL OR r.target_system = CAST($4 AS VARCHAR)) ORDER BY r.target_system, r.target_environment NULLS FIRST;

-- tumult-lake/src/duckdb_store/autopilot.rs:copy_sql("autopilot_decisions", "autopilot_decisions.parquet")
COPY autopilot_decisions TO 'autopilot_decisions.parquet' (FORMAT PARQUET);

-- tumult-lake/src/duckdb_store/autopilot.rs:copy_sql("autopilot_events", "autopilot_events.parquet")
COPY autopilot_events TO 'autopilot_events.parquet' (FORMAT PARQUET);

-- tumult-lake/src/duckdb_store/ingest.rs:delete_run_sql(activity_results)
DELETE FROM activity_results WHERE experiment_id = ?;

-- tumult-lake/src/duckdb_store/ingest.rs:delete_run_sql(load_results)
DELETE FROM load_results WHERE experiment_id = ?;

-- tumult-lake/src/duckdb_store/ingest.rs:delete_run_sql(probe_samples)
DELETE FROM probe_samples WHERE experiment_id = ?;

-- tumult-lake/src/duckdb_store/ingest.rs:delete_run_sql(nfr_verdicts)
DELETE FROM nfr_verdicts WHERE experiment_id = ?;

-- tumult-lake/src/duckdb_store/ingest.rs:delete_run_sql(graph_edges)
DELETE FROM graph_edges WHERE run_id = ?;

-- tumult-lake/src/duckdb_store/legacy.rs:attach_sql("legacy.duckdb")
-- Not linted: sqlfluff has no grammar for ATTACH.
-- ATTACH 'legacy.duckdb' AS legacy (READ_ONLY);

-- tumult-lake/src/duckdb_store/legacy.rs:detach_sql()
-- Not prepared: no legacy store is attached to a fresh lake.
-- Not linted: sqlfluff has no grammar for DETACH.
-- DETACH legacy;

-- tumult-lake/src/duckdb_store/legacy.rs:merge_sql("experiments", &[&id, &status], &["experiment_id"])
-- Not prepared: it reads the tables of a legacy store attached as legacy.
INSERT INTO experiments (experiment_id, status)
                 SELECT experiment_id, status FROM legacy.experiments src
                 WHERE NOT EXISTS (SELECT 1 FROM experiments cur WHERE cur.experiment_id IS NOT DISTINCT FROM src.experiment_id);

-- tumult-lake/src/duckdb_store/maintenance.rs:select_of(&experiments_schema(), "experiments")
SELECT experiment_id, title, status, started_at_ns, ended_at_ns, duration_ms, method_step_count, rollback_count, hypothesis_before_met, hypothesis_after_met, estimate_accuracy, resilience_score, test_kind, rto_target_s, rpo_target_s, rto_measured_s, rpo_measured_s, target_version, version_source, version_shares, impact_observed, outage_started_at_ns, recovered_at_ns, mttr_s, dora_scenarios FROM experiments;

-- tumult-lake/src/duckdb_store/maintenance.rs:select_of(&load_results_schema(), "load_results")
SELECT experiment_id, tool, started_at_ns, ended_at_ns, duration_s, vus, throughput_rps, latency_p50_ms, latency_p95_ms, latency_p99_ms, error_rate, total_requests, thresholds_met, aborted, truncated, planned_windows, completed_windows, unanswered_windows, lost_to_unanswered, sampled_duration_s, requests_sent, requests_ok, requests_timed_out, requests_transport, requests_http_4xx, requests_http_5xx, requests_http_other, requests_unclassified, censored_windows FROM load_results;

-- tumult-lake/src/duckdb_store/maintenance.rs:select_of(&journal_probe_samples_schema(), "probe_samples")
SELECT experiment_id, probe_name, phase, ts_ns, value, ok, reading FROM probe_samples;

-- tumult-lake/src/duckdb_store/maintenance.rs:roots_export_sql()
SELECT ts_ns, trace_id, span_id, parent_span_id, span_kind, duration_ns, status_code, status_message, service_name, service_version, experiment_id, experiment_name, outcome_status, target_system, target_technology, target_environment, span_attrs['resilience.target.version'] AS target_version, span_attrs['resilience.target.version_source'] AS version_source FROM spans WHERE span_name = 'resilience.experiment' AND experiment_id IS NOT NULL ORDER BY ts_ns, span_id;

-- tumult-lake/src/duckdb_store/maintenance.rs:SIGHTINGS_EXPORT
SELECT service_name, environment, service_version, min(first_seen_ns) AS first_seen_ns, max(last_seen_ns) AS last_seen_ns, max(coalesce(last_deployed_ns, first_seen_ns)) AS last_deployed_ns, CAST(max(coalesce(deployments, 1)) AS INTEGER) AS deployments, coalesce(bool_or(sanitized), false) AS sanitized FROM service_version_sightings GROUP BY service_name, environment, service_version ORDER BY service_name, first_seen_ns, service_version, environment;

-- tumult-lake/src/duckdb_store/maintenance.rs:stage_sql(ROOTS_STAGE)
CREATE OR REPLACE TEMP TABLE import_stage (ts_ns BIGINT, trace_id VARCHAR, span_id VARCHAR, parent_span_id VARCHAR, span_kind VARCHAR, duration_ns BIGINT, status_code VARCHAR, status_message VARCHAR, service_name VARCHAR, service_version VARCHAR, experiment_id VARCHAR, experiment_name VARCHAR, outcome_status VARCHAR, target_system VARCHAR, target_technology VARCHAR, target_environment VARCHAR, target_version VARCHAR, version_source VARCHAR);

-- tumult-lake/src/duckdb_store/maintenance.rs:merge_stage_sql(ROOTS_IMPORT)
-- Prepared after tumult-lake/src/duckdb_store/maintenance.rs:stage_sql(ROOTS_STAGE).
-- Not linted: sqlfluff has no grammar for the MAP literal.
-- INSERT INTO spans (ts_ns, trace_id, span_id, parent_span_id, span_kind, duration_ns, status_code, status_message, service_name, service_version, experiment_id, experiment_name, outcome_status, target_system, target_technology, target_environment, span_name, span_attrs, events) SELECT s.ts_ns, s.trace_id, s.span_id, s.parent_span_id, s.span_kind, s.duration_ns, s.status_code, s.status_message, s.service_name, s.service_version, s.experiment_id, s.experiment_name, s.outcome_status, s.target_system, s.target_technology, s.target_environment, 'resilience.experiment', CASE WHEN s.target_version IS NULL THEN NULL ELSE MAP {'resilience.target.version': s.target_version, 'resilience.target.version_source': s.version_source} END, '[]' FROM import_stage AS s WHERE NOT EXISTS (SELECT 1 FROM spans AS t WHERE t.trace_id = s.trace_id AND t.span_id = s.span_id); DROP TABLE import_stage;

-- tumult-lake/src/duckdb_store/maintenance.rs:stage_sql(SIGHTINGS_STAGE)
CREATE OR REPLACE TEMP TABLE import_stage (service_name VARCHAR, environment VARCHAR, service_version VARCHAR, first_seen_ns BIGINT, last_seen_ns BIGINT, last_deployed_ns BIGINT, deployments INTEGER, sanitized BOOLEAN);

-- tumult-lake/src/duckdb_store/maintenance.rs:merge_stage_sql(SIGHTINGS_IMPORT)
-- Prepared after tumult-lake/src/duckdb_store/maintenance.rs:stage_sql(SIGHTINGS_STAGE).
UPDATE service_version_sightings AS t SET first_seen_ns = least(t.first_seen_ns, s.first_seen_ns), last_seen_ns = greatest(t.last_seen_ns, s.last_seen_ns), last_deployed_ns = greatest(coalesce(t.last_deployed_ns, t.first_seen_ns), s.last_deployed_ns), deployments = greatest(coalesce(t.deployments, 1), s.deployments), sanitized = coalesce(t.sanitized, false) OR s.sanitized FROM import_stage AS s WHERE t.service_name = s.service_name AND t.service_version = s.service_version AND t.environment IS NOT DISTINCT FROM s.environment; INSERT INTO service_version_sightings (service_name, environment, service_version, first_seen_ns, last_seen_ns, last_deployed_ns, deployments, sanitized) SELECT s.service_name, s.environment, s.service_version, s.first_seen_ns, s.last_seen_ns, s.last_deployed_ns, s.deployments, s.sanitized FROM import_stage AS s WHERE NOT EXISTS (SELECT 1 FROM service_version_sightings AS t WHERE t.service_name = s.service_name AND t.service_version = s.service_version AND t.environment IS NOT DISTINCT FROM s.environment); DROP TABLE import_stage;

-- tumult-lake/src/duckdb_store/maintenance.rs:stage_like_sql(EXPERIMENTS.table)
CREATE OR REPLACE TEMP TABLE import_stage AS SELECT * FROM experiments LIMIT 0;

-- tumult-lake/src/duckdb_store/maintenance.rs:import_new_sql(&EXPERIMENTS)
-- Prepared after tumult-lake/src/duckdb_store/maintenance.rs:stage_like_sql(EXPERIMENTS.table).
INSERT INTO experiments SELECT * FROM import_stage AS s WHERE NOT EXISTS (SELECT 1 FROM experiments AS t WHERE t.experiment_id IS NOT DISTINCT FROM s.experiment_id); DROP TABLE import_stage;

-- tumult-lake/src/duckdb_store/maintenance.rs:stage_like_sql(ACTIVITIES.table)
CREATE OR REPLACE TEMP TABLE import_stage AS SELECT * FROM activity_results LIMIT 0;

-- tumult-lake/src/duckdb_store/maintenance.rs:import_new_sql(&ACTIVITIES)
-- Prepared after tumult-lake/src/duckdb_store/maintenance.rs:stage_like_sql(ACTIVITIES.table).
INSERT INTO activity_results SELECT * FROM import_stage AS s WHERE NOT EXISTS (SELECT 1 FROM activity_results AS t WHERE t.experiment_id IS NOT DISTINCT FROM s.experiment_id AND t.name IS NOT DISTINCT FROM s.name AND t.activity_type IS NOT DISTINCT FROM s.activity_type AND t.phase IS NOT DISTINCT FROM s.phase AND t.started_at_ns IS NOT DISTINCT FROM s.started_at_ns); DROP TABLE import_stage;

-- tumult-lake/src/duckdb_store/maintenance.rs:stage_like_sql(LOAD_RESULTS.table)
CREATE OR REPLACE TEMP TABLE import_stage AS SELECT * FROM load_results LIMIT 0;

-- tumult-lake/src/duckdb_store/maintenance.rs:import_new_sql(&LOAD_RESULTS)
-- Prepared after tumult-lake/src/duckdb_store/maintenance.rs:stage_like_sql(LOAD_RESULTS.table).
INSERT INTO load_results SELECT * FROM import_stage AS s WHERE NOT EXISTS (SELECT 1 FROM load_results AS t WHERE t.experiment_id IS NOT DISTINCT FROM s.experiment_id); DROP TABLE import_stage;

-- tumult-lake/src/duckdb_store/maintenance.rs:stage_like_sql(PROBE_SAMPLES.table)
CREATE OR REPLACE TEMP TABLE import_stage AS SELECT * FROM probe_samples LIMIT 0;

-- tumult-lake/src/duckdb_store/maintenance.rs:import_new_sql(&PROBE_SAMPLES)
-- Prepared after tumult-lake/src/duckdb_store/maintenance.rs:stage_like_sql(PROBE_SAMPLES.table).
INSERT INTO probe_samples SELECT * FROM import_stage AS s WHERE NOT EXISTS (SELECT 1 FROM probe_samples AS t WHERE t.experiment_id IS NOT DISTINCT FROM s.experiment_id AND t.probe_name IS NOT DISTINCT FROM s.probe_name AND t.phase IS NOT DISTINCT FROM s.phase AND t.ts_ns IS NOT DISTINCT FROM s.ts_ns); DROP TABLE import_stage;

-- tumult-lake/src/duckdb_store/recovery_trend.rs:RTO_SQL
WITH unrecovered AS (
    SELECT experiment_id, max(observed) AS watched_s
    FROM nfr_verdicts
    WHERE kind = 'rto' AND nfr_id = 'rto' AND recovered = false
    GROUP BY experiment_id
)
SELECT e.experiment_id, e.title, e.status, e.started_at_ns,
    CASE
        WHEN e.rto_measured_s IS NOT NULL THEN 'measured'
        WHEN u.watched_s IS NOT NULL THEN 'not_recovered'
        WHEN e.impact_observed = false THEN 'no_outage'
        ELSE 'not_measured'
    END AS point,
    coalesce(e.rto_measured_s, u.watched_s) AS value,
    e.rto_target_s AS target
FROM experiments AS e
LEFT JOIN unrecovered AS u ON u.experiment_id = e.experiment_id
WHERE e.rto_measured_s IS NOT NULL OR e.rto_target_s IS NOT NULL
ORDER BY e.started_at_ns, e.experiment_id;

-- tumult-lake/src/duckdb_store/recovery_trend.rs:RPO_SQL
SELECT e.experiment_id, e.title, e.status, e.started_at_ns,
    CASE WHEN e.rpo_measured_s IS NOT NULL THEN 'measured' ELSE 'not_measured' END AS point,
    e.rpo_measured_s AS value,
    e.rpo_target_s AS target
FROM experiments AS e
WHERE e.rpo_measured_s IS NOT NULL
    OR e.rpo_target_s IS NOT NULL
    OR EXISTS (
        SELECT 1 FROM nfr_verdicts AS v
        WHERE v.experiment_id = e.experiment_id AND v.kind = 'rpo'
    )
ORDER BY e.started_at_ns, e.experiment_id;

-- tumult-lake/src/entities/batch.rs:STAGING
CREATE TEMP TABLE IF NOT EXISTS entity_batch_keys (
    entity_key VARCHAR,
    environment VARCHAR
);
CREATE TEMP TABLE IF NOT EXISTS entity_batch_changes (
    entity_key VARCHAR,
    environment VARCHAR,
    valid_from_ns BIGINT,
    last_seen_ns BIGINT,
    last_received_ns BIGINT,
    wire_form VARCHAR,
    deleted_at_ns BIGINT,
    delete_reason VARCHAR
);
DELETE FROM entity_batch_keys;
DELETE FROM entity_batch_changes;

-- tumult-lake/src/entities/batch.rs:STAGE_KEYS
-- Prepared after tumult-lake/src/entities/batch.rs:STAGING.
INSERT INTO entity_batch_keys
SELECT u.entity_key, u.environment
FROM (
    SELECT unnest(from_json(CAST(? AS VARCHAR), '[{
        "entity_key": "VARCHAR", "environment": "VARCHAR"
    }]')) AS u
);

-- tumult-lake/src/entities/batch.rs:LOAD_LATEST
-- Prepared after tumult-lake/src/entities/batch.rs:STAGING.
SELECT entity_key, environment, valid_from_ns, last_seen_ns, interval_s,
       wire_form, state_hash, deleted_at_ns, delete_reason, observer,
       last_received_ns
FROM entity_versions
WHERE (entity_key, environment) IN (
    SELECT entity_key, environment FROM entity_batch_keys
)
QUALIFY row_number() OVER (
    PARTITION BY entity_key, environment ORDER BY valid_from_ns DESC
) = 1;

-- tumult-lake/src/entities/batch.rs:INSERT_VERSIONS
-- Prepared after tumult-lake/src/entities/batch.rs:STAGING.
INSERT INTO entity_versions (
    entity_key, environment, entity_type, entity_id, state_hash,
    description, relationships, valid_from_ns, last_seen_ns, interval_s,
    deleted_at_ns, delete_reason, wire_form, observer, schema_url,
    last_received_ns
)
SELECT u.entity_key, u.environment, u.entity_type, u.entity_id,
       u.state_hash, CAST(u.description AS JSON),
       CAST(u.relationships AS JSON), u.valid_from_ns, u.last_seen_ns,
       u.interval_s, u.deleted_at_ns, u.delete_reason, u.wire_form,
       u.observer, u.schema_url, u.last_received_ns
FROM (
    SELECT unnest(from_json(CAST(? AS VARCHAR), '[{
        "entity_key": "VARCHAR", "environment": "VARCHAR",
        "entity_type": "VARCHAR",
        "entity_id": "MAP(VARCHAR, VARCHAR)",
        "state_hash": "VARCHAR", "description": "VARCHAR",
        "relationships": "VARCHAR", "valid_from_ns": "BIGINT",
        "last_seen_ns": "BIGINT", "interval_s": "BIGINT",
        "deleted_at_ns": "BIGINT", "delete_reason": "VARCHAR",
        "wire_form": "VARCHAR", "observer": "VARCHAR",
        "schema_url": "VARCHAR", "last_received_ns": "BIGINT"
    }]')) AS u
);

-- tumult-lake/src/entities/batch.rs:STAGE_CHANGES
-- Prepared after tumult-lake/src/entities/batch.rs:STAGING.
INSERT INTO entity_batch_changes
SELECT u.entity_key, u.environment, u.valid_from_ns, u.last_seen_ns,
       u.last_received_ns, u.wire_form, u.deleted_at_ns, u.delete_reason
FROM (
    SELECT unnest(from_json(CAST(? AS VARCHAR), '[{
        "entity_key": "VARCHAR", "environment": "VARCHAR",
        "valid_from_ns": "BIGINT", "last_seen_ns": "BIGINT",
        "last_received_ns": "BIGINT", "wire_form": "VARCHAR",
        "deleted_at_ns": "BIGINT", "delete_reason": "VARCHAR"
    }]')) AS u
);

-- tumult-lake/src/entities/batch.rs:UPDATE_VERSIONS
-- Prepared after tumult-lake/src/entities/batch.rs:STAGING.
UPDATE entity_versions AS v
SET last_seen_ns = u.last_seen_ns,
    last_received_ns = u.last_received_ns,
    wire_form = u.wire_form,
    deleted_at_ns = u.deleted_at_ns,
    delete_reason = u.delete_reason
FROM entity_batch_changes AS u
WHERE v.entity_key = u.entity_key
    AND v.environment = u.environment
    AND v.valid_from_ns = u.valid_from_ns;

-- tumult-lake/src/entities/caps.rs:CENSUS
SELECT environment, count(DISTINCT entity_key)
FROM entity_versions
GROUP BY environment;

-- tumult-lake/src/entities/caps.rs:CAP_VERSIONS
-- Prepared after tumult-lake/src/entities/caps.rs:super::batch::STAGING.
DELETE FROM entity_versions WHERE rowid IN (
    SELECT rowid FROM entity_versions
    WHERE (entity_key, environment) IN (
        SELECT entity_key, environment FROM entity_batch_keys
    )
    QUALIFY row_number() OVER (
        PARTITION BY entity_key, environment ORDER BY valid_from_ns DESC
    ) > ?
);

-- tumult-lake/src/entities/caps.rs:VICTIMS
CREATE TEMP TABLE IF NOT EXISTS entity_victims (
    entity_key VARCHAR,
    environment VARCHAR,
    reason VARCHAR
);
DELETE FROM entity_victims;

-- tumult-lake/src/entities/caps.rs:STAGE_PRUNED
-- Prepared after tumult-lake/src/entities/caps.rs:VICTIMS.
INSERT INTO entity_victims
SELECT entity_key, environment, 'retention' FROM entities_current
WHERE liveness <> 'live' AND last_received_ns < ?
ORDER BY last_received_ns, entity_key, environment
LIMIT ?;

-- tumult-lake/src/entities/caps.rs:STAGE_PURGED
-- Prepared after tumult-lake/src/entities/caps.rs:VICTIMS.
INSERT INTO entity_victims
SELECT DISTINCT entity_key, environment, 'purge' FROM entity_versions
WHERE environment = ?
    AND (CAST(? AS VARCHAR) IS NULL OR entity_type = ?);

-- tumult-lake/src/entities/caps.rs:COUNT_VICTIMS
-- Prepared after tumult-lake/src/entities/caps.rs:VICTIMS.
SELECT environment, reason, count(*)
FROM entity_victims
GROUP BY environment, reason;

-- tumult-lake/src/entities/caps.rs:DELETE_VICTIMS
-- Prepared after tumult-lake/src/entities/caps.rs:VICTIMS.
DELETE FROM entity_versions WHERE (entity_key, environment) IN (
    SELECT entity_key, environment FROM entity_victims
);

-- tumult-lake/src/entities/caps.rs:STAGE_EVICTED
-- Prepared after tumult-lake/src/entities/caps.rs:super::batch::STAGING and tumult-lake/src/entities/caps.rs:VICTIMS.
INSERT INTO entity_victims
SELECT entity_key, environment,
       CASE liveness
           WHEN 'gone' THEN 'gone'
           WHEN 'expired' THEN 'expired'
           ELSE 'unreported'
       END
FROM entities_current
WHERE (CAST(? AS VARCHAR) IS NULL OR environment = ?)
    AND (liveness <> 'live' OR (interval_s = 0 AND last_received_ns < ?))
    AND (entity_key, environment) NOT IN (
        SELECT entity_key, environment FROM entity_batch_keys
    )
ORDER BY liveness = 'live', last_received_ns, entity_key, environment
LIMIT ?;

-- tumult-lake/src/ict.rs:REGISTER_FUNCTIONS_SQL
SELECT function_id, register_code, lei, name, licensed_activity, tier, reasons, last_assessed, rto_s, rpo_s, impact, org_unit, declared, test_plan, active FROM ict_functions ORDER BY lei, CAST(substr(register_code, 2) AS BIGINT);

-- tumult-lake/src/lake.rs:fingerprint_sql(spans)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'spans') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM spans t);

-- tumult-lake/src/lake.rs:fingerprint_sql(logs)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'logs') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM logs t);

-- tumult-lake/src/lake.rs:fingerprint_sql(metric_sums)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'metric_sums') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM metric_sums t);

-- tumult-lake/src/lake.rs:fingerprint_sql(metric_gauges)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'metric_gauges') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM metric_gauges t);

-- tumult-lake/src/lake.rs:fingerprint_sql(metric_histograms)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'metric_histograms') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM metric_histograms t);

-- tumult-lake/src/lake.rs:fingerprint_sql(manual_experiment_audit)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'manual_experiment_audit') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM manual_experiment_audit t);

-- tumult-lake/src/lake.rs:fingerprint_sql(manual_experiments)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'manual_experiments') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM manual_experiments t);

-- tumult-lake/src/lake.rs:fingerprint_sql(evidence_attachments)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'evidence_attachments') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM evidence_attachments t);

-- tumult-lake/src/lake.rs:fingerprint_sql(manual_exercise_steps)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'manual_exercise_steps') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM manual_exercise_steps t);

-- tumult-lake/src/lake.rs:fingerprint_sql(manual_action_items)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'manual_action_items') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM manual_action_items t);

-- tumult-lake/src/lake.rs:fingerprint_sql(manual_experiment_links)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'manual_experiment_links') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM manual_experiment_links t);

-- tumult-lake/src/lake.rs:fingerprint_sql(manual_exercise_plans)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'manual_exercise_plans') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM manual_exercise_plans t);

-- tumult-lake/src/lake.rs:fingerprint_sql(manual_plan_audit)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'manual_plan_audit') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM manual_plan_audit t);

-- tumult-lake/src/lake.rs:fingerprint_sql(manual_signoffs)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'manual_signoffs') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM manual_signoffs t);

-- tumult-lake/src/lake.rs:fingerprint_sql(import_batches)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'import_batches') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM import_batches t);

-- tumult-lake/src/lake.rs:fingerprint_sql(experiments)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'experiments') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM experiments t);

-- tumult-lake/src/lake.rs:fingerprint_sql(activity_results)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'activity_results') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM activity_results t);

-- tumult-lake/src/lake.rs:fingerprint_sql(load_results)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'load_results') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM load_results t);

-- tumult-lake/src/lake.rs:fingerprint_sql(probe_samples)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'probe_samples') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM probe_samples t);

-- tumult-lake/src/lake.rs:fingerprint_sql(nfr_verdicts)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'nfr_verdicts') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM nfr_verdicts t);

-- tumult-lake/src/lake.rs:fingerprint_sql(perf_verdicts)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'perf_verdicts') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM perf_verdicts t);

-- tumult-lake/src/lake.rs:fingerprint_sql(forecasts)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'forecasts') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM forecasts t);

-- tumult-lake/src/lake.rs:fingerprint_sql(autopilot_decisions)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'autopilot_decisions') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM autopilot_decisions t);

-- tumult-lake/src/lake.rs:fingerprint_sql(autopilot_events)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'autopilot_events') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM autopilot_events t);

-- tumult-lake/src/lake.rs:fingerprint_sql(autopilot_change_events)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'autopilot_change_events') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM autopilot_change_events t);

-- tumult-lake/src/lake.rs:fingerprint_sql(graph_nodes)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'graph_nodes') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM graph_nodes t);

-- tumult-lake/src/lake.rs:fingerprint_sql(graph_edges)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'graph_edges') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM graph_edges t);

-- tumult-lake/src/lake.rs:fingerprint_sql(agentic_runs)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'agentic_runs') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM agentic_runs t);

-- tumult-lake/src/lake.rs:fingerprint_sql(agentic_contract_outcomes)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'agentic_contract_outcomes') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM agentic_contract_outcomes t);

-- tumult-lake/src/lake.rs:fingerprint_sql(agentic_fault_applications)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'agentic_fault_applications') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM agentic_fault_applications t);

-- tumult-lake/src/lake.rs:fingerprint_sql(agentic_replay_outcomes)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'agentic_replay_outcomes') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM agentic_replay_outcomes t);

-- tumult-lake/src/lake.rs:fingerprint_sql(runs)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'runs') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM runs t);

-- tumult-lake/src/lake.rs:fingerprint_sql(run_registry)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'run_registry') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM run_registry t);

-- tumult-lake/src/lake.rs:fingerprint_sql(run_audit)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'run_audit') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM run_audit t);

-- tumult-lake/src/lake.rs:fingerprint_sql(run_schedules)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'run_schedules') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM run_schedules t);

-- tumult-lake/src/lake.rs:fingerprint_sql(approval_requests)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'approval_requests') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM approval_requests t);

-- tumult-lake/src/lake.rs:fingerprint_sql(approval_decisions)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'approval_decisions') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM approval_decisions t);

-- tumult-lake/src/lake.rs:fingerprint_sql(webhook_cursors)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'webhook_cursors') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM webhook_cursors t);

-- tumult-lake/src/lake.rs:fingerprint_sql(webhook_dead_letters)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'webhook_dead_letters') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM webhook_dead_letters t);

-- tumult-lake/src/lake.rs:fingerprint_sql(risk_suppressions)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'risk_suppressions') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM risk_suppressions t);

-- tumult-lake/src/lake.rs:fingerprint_sql(ict_legal_entities)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'ict_legal_entities') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM ict_legal_entities t);

-- tumult-lake/src/lake.rs:fingerprint_sql(ict_functions)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'ict_functions') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM ict_functions t);

-- tumult-lake/src/lake.rs:fingerprint_sql(ict_function_services)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'ict_function_services') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM ict_function_services t);

-- tumult-lake/src/lake.rs:fingerprint_sql(requirement_status_daily)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'requirement_status_daily') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM requirement_status_daily t);

-- tumult-lake/src/lake.rs:fingerprint_sql(maturity_daily)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'maturity_daily') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM maturity_daily t);

-- tumult-lake/src/lake.rs:fingerprint_sql(entity_versions)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'entity_versions') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM entity_versions t);

-- tumult-lake/src/lake.rs:fingerprint_sql(service_version_sightings)
SELECT sha256( (SELECT COALESCE(string_agg(sha256(CAST(row_to_json(c) AS VARCHAR)), ',' ORDER BY ordinal_position), '') FROM (SELECT column_name, data_type, is_nullable, column_default, ordinal_position FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'service_version_sightings') c) || ':' || COALESCE(string_agg(h, ',' ORDER BY h), '')) AS fp FROM (SELECT sha256(CAST(row_to_json(t) AS VARCHAR)) AS h FROM service_version_sightings t);

-- tumult-lake/src/lake.rs:count_sql("runs")
SELECT count(*) AS n FROM runs;

-- tumult-lake/src/lake.rs:copy_sql("runs", "snapshots/runs/fingerprint.parquet.tmp")
COPY (SELECT * FROM runs) TO 'snapshots/runs/fingerprint.parquet.tmp' (FORMAT PARQUET);

-- tumult-lake/src/manual/chain.rs:Trail::Record.insert_sql()
INSERT INTO manual_experiment_audit (
                    id, experiment_id, changed_by, changed_at_ns, action, diff,
                    prev_hash, new_hash, link_hash
                 ) VALUES (?,?,?,?,?,CAST(? AS JSON),?,?,?);

-- tumult-lake/src/manual/chain.rs:Trail::Plan.insert_sql()
INSERT INTO manual_plan_audit (
                    id, plan_id, changed_by, changed_at_ns, action, diff,
                    prev_hash, new_hash, link_hash
                 ) VALUES (?,?,?,?,?,CAST(? AS JSON),?,?,?);

-- tumult-lake/src/manual/chain.rs:Trail::Record.rows_sql()
SELECT id, experiment_id AS subject_id, changed_by, changed_at_ns, action, CAST(diff AS VARCHAR) AS diff, prev_hash, new_hash, link_hash FROM manual_experiment_audit WHERE experiment_id = ? ORDER BY changed_at_ns ASC, id ASC;

-- tumult-lake/src/manual/chain.rs:Trail::Plan.rows_sql()
SELECT id, plan_id AS subject_id, changed_by, changed_at_ns, action, CAST(diff AS VARCHAR) AS diff, prev_hash, new_hash, link_hash FROM manual_plan_audit WHERE plan_id = ? ORDER BY changed_at_ns ASC, id ASC;

-- tumult-lake/src/manual/chain.rs:Trail::Record.last_sql()
SELECT link_hash, changed_at_ns AS at_ns FROM manual_experiment_audit WHERE experiment_id = ? ORDER BY changed_at_ns DESC, id DESC LIMIT 1;

-- tumult-lake/src/manual/chain.rs:Trail::Plan.last_sql()
SELECT link_hash, changed_at_ns AS at_ns FROM manual_plan_audit WHERE plan_id = ? ORDER BY changed_at_ns DESC, id DESC LIMIT 1;

-- tumult-lake/src/manual/chain.rs:SIGNOFFS_SQL
SELECT * FROM manual_signoffs WHERE subject_kind = ? AND subject_id = ? ORDER BY signed_at_ns ASC, id ASC;

-- tumult-lake/src/manual/chain.rs:LAST_SIGNOFF_SQL
SELECT link_hash, signed_at_ns AS at_ns FROM manual_signoffs WHERE subject_kind = ? AND subject_id = ? ORDER BY signed_at_ns DESC, id DESC LIMIT 1;

-- tumult-lake/src/manual/files.rs:FILE_HASHES_SQL
SELECT DISTINCT file_hash AS h FROM evidence_attachments WHERE kind = 'file' AND file_hash IS NOT NULL ORDER BY h;

-- tumult-lake/src/manual/plans.rs:plans_sql(&PlanFilter::default()).0
SELECT * FROM manual_exercise_plans WHERE TRUE ORDER BY due_at_ns ASC, id ASC;

-- tumult-lake/src/manual/plans.rs:plans_sql(&every).0
SELECT * FROM manual_exercise_plans WHERE TRUE AND status = ? AND target_system = ? AND due_at_ns < ? ORDER BY due_at_ns ASC, id ASC;

-- tumult-lake/src/manual/reader.rs:STEPS_SQL
SELECT ordinal, title, actor, started_at_ns, ended_at_ns, result, note FROM manual_exercise_steps WHERE experiment_id = ? ORDER BY ordinal;

-- tumult-lake/src/manual/reader.rs:ACTIONS_SQL
SELECT id, ordinal, title, owner, due_at_ns, ticket, status, closed_at_ns, closed_by, severity, retest_kind, retest_id FROM manual_action_items WHERE experiment_id = ? ORDER BY ordinal;

-- tumult-lake/src/manual/reader.rs:LINKS_SQL
SELECT ordinal, kind, linked_id, label FROM manual_experiment_links WHERE experiment_id = ? ORDER BY ordinal;

-- tumult-lake/src/manual/writer.rs:DELETE_CHILDREN[0]
DELETE FROM manual_exercise_steps WHERE experiment_id = ?;

-- tumult-lake/src/manual/writer.rs:DELETE_CHILDREN[1]
DELETE FROM manual_action_items WHERE experiment_id = ?;

-- tumult-lake/src/manual/writer.rs:DELETE_CHILDREN[2]
DELETE FROM manual_experiment_links WHERE experiment_id = ?;

-- tumult-lake/src/perf.rs:SAME_AS_LATEST
SELECT count(*) FROM (
    SELECT * FROM perf_verdicts
    WHERE target = ? AND probe = ? AND baseline_version = ?
      AND candidate_version = ? AND quantile = ?
      AND CAST(envs AS VARCHAR) IS NOT DISTINCT FROM ?
    ORDER BY computed_at_ns DESC, rowid DESC LIMIT 1
) AS latest
WHERE latest.baseline_n = ? AND latest.candidate_n = ?
  AND latest.baseline_p50 IS NOT DISTINCT FROM ?
  AND latest.baseline_p95 IS NOT DISTINCT FROM ?
  AND latest.baseline_p99 IS NOT DISTINCT FROM ?
  AND latest.candidate_p50 IS NOT DISTINCT FROM ?
  AND latest.candidate_p95 IS NOT DISTINCT FROM ?
  AND latest.candidate_p99 IS NOT DISTINCT FROM ?
  AND latest.delta IS NOT DISTINCT FROM ?
  AND latest.delta_pct IS NOT DISTINCT FROM ?
  AND latest.ci_low IS NOT DISTINCT FROM ?
  AND latest.ci_high IS NOT DISTINCT FROM ?
  AND latest.confidence = ? AND latest.margin = ? AND latest.verdict = ?;

-- tumult-lake/src/scope.rs:and(in_list("target_environment", &envs))
SELECT count(*) AS n FROM run_roots WHERE TRUE AND target_environment IN ('prod', 'o''hare');

-- tumult-lake/src/scope.rs:and(table_predicate("metric_sums", &envs))
SELECT count(*) AS n FROM metric_sums WHERE TRUE AND EXISTS (SELECT 1 FROM spans se WHERE se.experiment_name = metric_sums.experiment_name AND se.target_environment IN ('prod', 'o''hare'));

-- tumult-lake/src/sightings.rs:HELD
SELECT
    environment,
    service_version,
    min(first_seen_ns) AS first_seen_ns,
    max(last_seen_ns) AS last_seen_ns,
    max(coalesce(last_deployed_ns, first_seen_ns)) AS last_deployed_ns,
    max(coalesce(deployments, 1)) AS deployments,
    coalesce(bool_or(sanitized), false) AS sanitized,
    min(coalesce(first_anchor_ns, first_seen_ns)) AS first_anchor_ns
FROM service_version_sightings
WHERE service_name = ?
GROUP BY environment, service_version
ORDER BY first_seen_ns, service_version, environment;

-- tumult-lake/src/store.rs:json_rows_sql("SELECT id, state FROM runs")
SELECT row_to_json(t) AS j FROM (SELECT id, state FROM runs) AS t;
