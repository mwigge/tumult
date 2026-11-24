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
CREATE INDEX IF NOT EXISTS idx_probe_samples_experiment_id ON probe_samples (experiment_id);
CREATE TABLE IF NOT EXISTS nfr_verdicts (
    experiment_id VARCHAR NOT NULL, nfr_id VARCHAR NOT NULL,
    kind VARCHAR NOT NULL, metric VARCHAR NOT NULL,
    operator VARCHAR NOT NULL, threshold DOUBLE NOT NULL,
    observed DOUBLE, state VARCHAR NOT NULL, margin DOUBLE,
    criticality VARCHAR NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_nfr_verdicts_experiment_id ON nfr_verdicts (experiment_id);
";

/// v23: `probe_samples.reading`, the number a probe printed when its
/// tolerance reads one. Added last, as the appender writes by position, and
/// NULL on the rows stored before. `value` keeps its meaning.
pub(super) const READING_COLUMN: &str =
    "ALTER TABLE probe_samples ADD COLUMN IF NOT EXISTS reading DOUBLE;";

/// v24: what a DR run measured, as its journal says it. `experiments`
/// gains whether the probes saw an outage, when it started, when the
/// target was back and the time to recovery (`post_result.mttr_s`).
/// `nfr_verdicts` gains whether the run recovered and the reason of the
/// verdict. Added last, as the appender writes by position, after the
/// columns `perf.rs` adds to `experiments`, and NULL on the rows stored
/// before.
pub(super) const DR_FACTS_COLUMNS: &str = "
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS impact_observed BOOLEAN;
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS outage_started_at_ns BIGINT;
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS recovered_at_ns BIGINT;
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS mttr_s DOUBLE;
ALTER TABLE nfr_verdicts ADD COLUMN IF NOT EXISTS recovered BOOLEAN;
ALTER TABLE nfr_verdicts ADD COLUMN IF NOT EXISTS reason VARCHAR;
";

/// v26: the plan scenarios of RTS 2024/1774 a run names, from the
/// `scenarios` of its regulatory block, as a JSON list of ids, the way
/// `version_shares` holds a list: the CSV export of a journal writes no
/// list column. Added last, as the appender writes by position, and NULL on
/// the rows stored before and on a run that names none.
pub(super) const SCENARIOS_COLUMN: &str = "
ALTER TABLE experiments ADD COLUMN IF NOT EXISTS dora_scenarios VARCHAR;
";
