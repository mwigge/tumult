//! Analytics backend trait — abstraction over `DuckDB` and `ClickHouse`.
//!
//! Both backends implement [`AnalyticsBackend`], allowing the CLI and MCP
//! to swap between embedded `DuckDB` and external `ClickHouse` transparently.

use tumult_core::types::Journal;

use crate::duckdb_store::StoreStats;
use crate::error::AnalyticsError;

#[doc(hidden)]
pub mod private {
    /// Sealed supertrait to prevent external implementations of `AnalyticsBackend`.
    pub trait Sealed {}
}

/// Unified interface for analytics storage backends.
///
/// This trait is sealed -- it cannot be implemented outside this crate.
/// Use the provided `AnalyticsStore` (`DuckDB`) or `ClickHouseStore` backends.
///
/// Implemented by:
/// - [`crate::duckdb_store::AnalyticsStore`] -- embedded, zero-dependency (default)
/// - `tumult_clickhouse::ClickHouseStore` -- external, shared with `SigNoz`
pub trait AnalyticsBackend: private::Sealed {
    /// Ingest a journal. Returns true if new, false if duplicate.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying store operation fails.
    fn ingest_journal(&self, journal: &Journal) -> Result<bool, AnalyticsError>;

    /// Ingest multiple journals, skipping duplicates. Returns count of new.
    ///
    /// # Errors
    ///
    /// Returns an error if any individual journal ingestion fails.
    fn ingest_journals(&self, journals: &[Journal]) -> Result<usize, AnalyticsError> {
        let mut count = 0;
        for journal in journals {
            if self.ingest_journal(journal)? {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Execute a SQL query. Returns rows as stringified values.
    ///
    /// # Errors
    ///
    /// Returns an error if the SQL query fails to execute.
    fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, AnalyticsError>;

    /// Get column names for a SQL query.
    ///
    /// # Errors
    ///
    /// Returns an error if the SQL query fails to execute.
    fn query_columns(&self, sql: &str) -> Result<Vec<String>, AnalyticsError>;

    /// Count experiments in the store.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying count query fails.
    fn experiment_count(&self) -> Result<usize, AnalyticsError>;

    /// Get store statistics.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying statistics query fails.
    fn stats(&self) -> Result<StoreStats, AnalyticsError>;

    /// Purge experiments older than N days. Returns count removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the purge operation fails.
    fn purge_older_than_days(&self, days: u32) -> Result<usize, AnalyticsError>;

    /// Schema version for migration tracking.
    ///
    /// # Errors
    ///
    /// Returns an error if the schema version cannot be read.
    fn schema_version(&self) -> Result<i64, AnalyticsError>;
}

impl private::Sealed for crate::duckdb_store::AnalyticsStore {}

// Implement AnalyticsBackend for the existing DuckDB store.
impl AnalyticsBackend for crate::duckdb_store::AnalyticsStore {
    fn ingest_journal(&self, journal: &Journal) -> Result<bool, AnalyticsError> {
        self.ingest_journal(journal)
    }

    fn ingest_journals(&self, journals: &[Journal]) -> Result<usize, AnalyticsError> {
        self.ingest_journals(journals)
    }

    fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, AnalyticsError> {
        self.query(sql)
    }

    fn query_columns(&self, sql: &str) -> Result<Vec<String>, AnalyticsError> {
        self.query_columns(sql)
    }

    fn experiment_count(&self) -> Result<usize, AnalyticsError> {
        self.experiment_count()
    }

    fn stats(&self) -> Result<StoreStats, AnalyticsError> {
        self.stats()
    }

    fn purge_older_than_days(&self, days: u32) -> Result<usize, AnalyticsError> {
        self.purge_older_than_days(days)
    }

    fn schema_version(&self) -> Result<i64, AnalyticsError> {
        self.schema_version()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duckdb_implements_backend_trait() {
        let store = crate::duckdb_store::AnalyticsStore::in_memory().unwrap();
        // Use via trait
        let backend: &dyn AnalyticsBackend = &store;
        assert_eq!(backend.experiment_count().unwrap(), 0);
        assert_eq!(backend.schema_version().unwrap(), 1);
        let stats = backend.stats().unwrap();
        assert_eq!(stats.experiment_count, 0);
        assert_eq!(stats.activity_count, 0);
    }

    #[test]
    fn duckdb_backend_ingest_and_query() {
        use tumult_core::types::*;

        let store = crate::duckdb_store::AnalyticsStore::in_memory().unwrap();
        let backend: &dyn AnalyticsBackend = &store;

        let journal = Journal {
            experiment_title: "trait test".into(),
            experiment_id: "bt-001".into(),
            status: ExperimentStatus::Completed,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_060_000_000_000,
            duration_ms: 60_000,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![],
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
        };

        assert!(backend.ingest_journal(&journal).unwrap());
        assert!(!backend.ingest_journal(&journal).unwrap()); // duplicate
        assert_eq!(backend.experiment_count().unwrap(), 1);

        let rows = backend
            .query("SELECT experiment_id FROM experiments")
            .unwrap();
        assert_eq!(rows[0][0], "bt-001");

        let cols = backend
            .query_columns("SELECT experiment_id, status FROM experiments")
            .unwrap();
        assert_eq!(cols, vec!["experiment_id", "status"]);
    }
}
