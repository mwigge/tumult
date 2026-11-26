//! The statements of the entity versions (`entities/`).

pub(crate) const COUNT_VERSIONS: &str = "SELECT count(*) FROM entity_versions";

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![COUNT_VERSIONS]
}
