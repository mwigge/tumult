//! v3 analytics DDL: the tumult-analytics family, unified into this
//! database under unchanged table names so TUI/MCP/CLI SQL keeps working.
//! `experiments` stays the authoritative journal-detail table;
//! `experiment_runs` (the span-rollup view above) joins to it by
//! experiment id.

/// Journal detail, the four `agentic_*` tables, and the autopilot decision
/// store (INSERT-ONLY, event-sourcing style).
pub(super) const DDL: &str = "
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
";
