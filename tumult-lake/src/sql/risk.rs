//! The statements of the accepted risks (`risk.rs`).

pub(crate) const LAST_SUPPRESSION_HASH: &str =
    "SELECT new_hash FROM risk_suppressions WHERE risk_id = ? \
    ORDER BY at_ns DESC, rowid DESC LIMIT 1";

pub(crate) const INSERT_SUPPRESSION: &str = "INSERT INTO risk_suppressions \
    (id, risk_id, reason, actor, at_ns, expires_at_ns, prev_hash, new_hash) \
    VALUES (?,?,?,?,?,?,?,?)";

pub(crate) const SUPPRESSIONS: &str =
    "SELECT * FROM risk_suppressions ORDER BY at_ns ASC, rowid ASC";

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![LAST_SUPPRESSION_HASH, INSERT_SUPPRESSION, SUPPRESSIONS]
}
