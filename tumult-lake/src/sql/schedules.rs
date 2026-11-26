//! The statements of the run schedules (`schedules.rs`).

pub(crate) const INSERT: &str = "INSERT INTO run_schedules VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)";

pub(crate) const SET_ENABLED: &str = "UPDATE run_schedules SET enabled = ? WHERE id = ?";

pub(crate) const DELETE: &str = "DELETE FROM run_schedules WHERE id = ?";

pub(crate) const FIRED: &str = "UPDATE run_schedules \
    SET last_run_at_ns = ?, last_run_id = ?, next_run_at_ns = ? WHERE id = ?";

pub(crate) const LIST: &str = "SELECT * FROM run_schedules ORDER BY name";

/// The enabled schedules due at or before `now_ns`, oldest first.
pub(crate) fn due(now_ns: i64) -> String {
    format!(
        "SELECT * FROM run_schedules WHERE enabled AND next_run_at_ns <= {now_ns} \
         ORDER BY next_run_at_ns"
    )
}

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        INSERT,
        SET_ENABLED,
        DELETE,
        FIRED,
        LIST,
        due(1_774_980_000_000_000_000)
    ]
}
