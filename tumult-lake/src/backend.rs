//! Analytics backend trait — abstraction over `DuckDB` and `ClickHouse`.
//!
//! Both backends implement [`AnalyticsBackend`], allowing the CLI and MCP
//! to swap between embedded `DuckDB` and external `ClickHouse` transparently.

use tumult_core::types::Journal;

use crate::error::AnalyticsError;
use crate::query_row::QueryRow;

/// Aggregate statistics reported by an analytics backend.
///
/// Also re-exported as `crate::duckdb_store::StoreStats` (with the `duckdb`
/// feature enabled) for backwards compatibility.
pub struct StoreStats {
    pub experiment_count: usize,
    pub activity_count: usize,
}

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
/// - `crate::duckdb_store::AnalyticsStore` -- embedded, zero-dependency
///   (default, requires the `duckdb` feature)
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
    fn query(&self, sql: &str) -> Result<Vec<QueryRow>, AnalyticsError>;

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

#[cfg(feature = "duckdb")]
impl private::Sealed for crate::duckdb_store::AnalyticsStore {}

// Implement AnalyticsBackend for the existing DuckDB store.
#[cfg(feature = "duckdb")]
impl AnalyticsBackend for crate::duckdb_store::AnalyticsStore {
    fn ingest_journal(&self, journal: &Journal) -> Result<bool, AnalyticsError> {
        self.ingest_journal(journal)
    }

    fn ingest_journals(&self, journals: &[Journal]) -> Result<usize, AnalyticsError> {
        self.ingest_journals(journals)
    }

    fn query(&self, sql: &str) -> Result<Vec<QueryRow>, AnalyticsError> {
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

#[cfg(all(test, feature = "duckdb"))]
mod tests {
    use super::*;

    #[test]
    fn duckdb_implements_backend_trait() {
        let store = crate::duckdb_store::AnalyticsStore::in_memory().unwrap();
        // Use via trait
        let backend: &dyn AnalyticsBackend = &store;
        assert_eq!(backend.experiment_count().unwrap(), 0);
        assert_eq!(
            backend.schema_version().unwrap(),
            crate::CURRENT_SCHEMA_VERSION
        );
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
            halt: None,
            blast_radius: None,
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

    #[test]
    fn duckdb_backend_ingest_journals_counts_only_new() {
        use tumult_core::types::*;

        let store = crate::duckdb_store::AnalyticsStore::in_memory().unwrap();
        let backend: &dyn AnalyticsBackend = &store;
        let journal = |id: &str| Journal {
            experiment_title: format!("batch {id}"),
            experiment_id: id.into(),
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
            halt: None,
            blast_radius: None,
        };
        // First pass: all three are new. Second pass: all duplicates.
        let batch = [journal("bj-1"), journal("bj-2"), journal("bj-3")];
        assert_eq!(backend.ingest_journals(&batch).unwrap(), 3);
        assert_eq!(backend.ingest_journals(&batch).unwrap(), 0);
        assert_eq!(backend.experiment_count().unwrap(), 3);
    }

    #[test]
    fn duckdb_backend_purge_and_bad_sql_error_paths() {
        let store = crate::duckdb_store::AnalyticsStore::in_memory().unwrap();
        let backend: &dyn AnalyticsBackend = &store;
        // Purging an empty store removes nothing.
        assert_eq!(backend.purge_older_than_days(30).unwrap(), 0);
        // Invalid SQL surfaces as an error, not a panic.
        assert!(backend.query("SELECT FROM nonsense").is_err());
        assert!(backend.query_columns("SELECT FROM nonsense").is_err());
    }
}
