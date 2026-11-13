//! v18 performance DDL: `perf_verdicts`, one row per regression verdict
//! computed for a target between two `service.version`s. Additive and
//! index-free apart from the per-target lookup index, under the same
//! crash-robustness rule as the v5 run tables; a recomputation appends a
//! new row (`computed_at_ns`) rather than updating in place. `envs` is the
//! environment list the evidence was read under (NULL when unscoped), so
//! history can be filtered by a principal's scopes.

/// `perf_verdicts`: the two versions, the probe series compared, the
/// per-side percentiles, the bootstrap interval and the verdict.
pub(super) const DDL: &str = "
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
";
