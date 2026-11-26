//! The statements of the performance verdicts (`perf.rs`).

pub(crate) const INSERT_VERDICT: &str = "INSERT INTO perf_verdicts (
       target, probe, baseline_version, candidate_version, quantile,
       baseline_n, candidate_n,
       baseline_p50, baseline_p95, baseline_p99,
       candidate_p50, candidate_p95, candidate_p99,
       delta, delta_pct, ci_low, ci_high, confidence, margin,
       verdict, reasons, computed_at_ns, envs
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";

pub(crate) const VERDICTS: &str =
    "SELECT target, probe, baseline_version, candidate_version, quantile,
           baseline_n, candidate_n,
           baseline_p50, baseline_p95, baseline_p99,
           candidate_p50, candidate_p95, candidate_p99,
           delta, delta_pct, ci_low, ci_high, confidence, margin,
           verdict, CAST(reasons AS VARCHAR), computed_at_ns,
           CAST(envs AS VARCHAR)
    FROM perf_verdicts WHERE target = ?
      AND (? OR (envs IS NOT NULL AND list_has_all(
           CAST(CAST(? AS JSON) AS VARCHAR[]), CAST(envs AS VARCHAR[]))))
    ORDER BY computed_at_ns DESC, rowid DESC LIMIT ?";

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![INSERT_VERDICT, VERDICTS]
}
