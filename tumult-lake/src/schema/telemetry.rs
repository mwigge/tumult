//! v1 telemetry DDL: the wide OTLP tables (`spans`, `logs`, `metric_*`),
//! `import_batches` and the `schema_meta` version store.

/// v1 tables plus the v1.1 promoted `metric_histograms` columns (the CREATE
/// already includes them; `ADD COLUMN IF NOT EXISTS` makes the ALTER a
/// no-op there).
pub(super) const DDL: &str = "
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
