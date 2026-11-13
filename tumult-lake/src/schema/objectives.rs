//! v16 objectives DDL: the resilience discipline and recovery targets on
//! `experiments`, raw probe samples, and per-run objective verdicts.
//! Additive (ALTER … ADD COLUMN IF NOT EXISTS, CREATE TABLE IF NOT EXISTS)
//! and index-free apart from the per-experiment lookup indexes, under the
//! same crash-robustness rule as the v5 run tables.

/// `experiments` gains `test_kind`, `rto_target_s`, `rpo_target_s`,
/// `rto_measured_s`, `rpo_measured_s`; `probe_samples` keeps every raw
/// probe observation of a run; `nfr_verdicts` keeps one row per objective
/// evaluated against a run.
pub(super) const DDL: &str = "
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
CREATE INDEX IF NOT EXISTS idx_probe_samples_experiment_id
    ON probe_samples (experiment_id);
CREATE TABLE IF NOT EXISTS nfr_verdicts (
    experiment_id VARCHAR NOT NULL, nfr_id VARCHAR NOT NULL,
    kind VARCHAR NOT NULL, metric VARCHAR NOT NULL,
    operator VARCHAR NOT NULL, threshold DOUBLE NOT NULL,
    observed DOUBLE, state VARCHAR NOT NULL, margin DOUBLE,
    criticality VARCHAR NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_nfr_verdicts_experiment_id
    ON nfr_verdicts (experiment_id);
";
