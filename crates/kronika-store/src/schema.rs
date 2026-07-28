//! Schema v1 DDL.
//!
//! Wide, ClickHouse-exporter-aligned tables plus `MAP(VARCHAR, VARCHAR)`
//! attribute maps for the dynamic tail (e.g. `resilience.baseline.probe.{name}.*`).
//! Low-cardinality, high-selectivity `resilience.*` keys are materialized as
//! columns by the ingest layer; everything else stays in the maps.

pub const CURRENT_SCHEMA_VERSION: i64 = 1;

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
";

/// Rollup view: one row per experiment run, over the experiment root spans
/// tumult emits as `resilience.experiment`.
pub const CREATE_VIEWS: &str = "
CREATE VIEW IF NOT EXISTS experiment_runs AS
SELECT
    experiment_id,
    any_value(experiment_name) AS experiment_name,
    min(ts_ns) AS started_at_ns,
    max(ts_ns + duration_ns) AS ended_at_ns,
    max(duration_ns) AS duration_ns,
    any_value(outcome_status) AS outcome_status,
    any_value(hypothesis_met) AS hypothesis_met
FROM spans
WHERE span_name = 'resilience.experiment'
GROUP BY experiment_id;
";
