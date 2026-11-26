//! The statements of the reader of the experiment runs (`reader.rs`).

pub(crate) const EXPERIMENT_RUNS: &str =
    "SELECT experiment_id, experiment_name, started_at_ns, ended_at_ns,
           duration_ns, outcome_status, hypothesis_met
    FROM experiment_runs ORDER BY started_at_ns";

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![EXPERIMENT_RUNS]
}
