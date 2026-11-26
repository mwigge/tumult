//! The statements of the store itself: the schema version and the rows
//! the migration adds (`store.rs`, `writer.rs`, `backup.rs`).

pub(crate) const SCHEMA_META_EXISTS: &str =
    "SELECT count(*) > 0 FROM information_schema.tables WHERE table_name = 'schema_meta'";

pub(crate) const NEWEST_SCHEMA_VERSION: &str =
    "SELECT max(value) FROM schema_meta WHERE key = 'version'";

pub(crate) const SCHEMA_VERSION: &str = "SELECT value FROM schema_meta WHERE key = 'version'";

pub(crate) const INSERT_LEGACY_USER: &str =
    "INSERT INTO users VALUES ('legacy', 'legacy', '!', 'viewer', false, true, 0)";

pub(crate) const INSERT_SCHEMA_VERSION: &str =
    "INSERT INTO schema_meta (key, value) VALUES ('version', ?)";

pub(crate) const UPDATE_SCHEMA_VERSION: &str =
    "UPDATE schema_meta SET value = ? WHERE key = 'version'";

pub(crate) const SCHEMA_VERSION_AS_V: &str =
    "SELECT value AS v FROM schema_meta WHERE key = 'version'";

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        SCHEMA_META_EXISTS,
        NEWEST_SCHEMA_VERSION,
        SCHEMA_VERSION,
        SCHEMA_VERSION_AS_V,
        INSERT_LEGACY_USER,
        INSERT_SCHEMA_VERSION,
        UPDATE_SCHEMA_VERSION
    ]
}
