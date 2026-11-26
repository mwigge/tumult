//! The statements that open and end a transaction.

pub(crate) const BEGIN: &str = "BEGIN TRANSACTION";

pub(crate) const COMMIT: &str = "COMMIT";

pub(crate) const ROLLBACK: &str = "ROLLBACK";

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![BEGIN, COMMIT, ROLLBACK]
}
