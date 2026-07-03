//! Synchronous `AnalyticsBackend` trait implementation for [`ClickHouseStore`].

use tumult_analytics::backend::AnalyticsBackend;
use tumult_analytics::duckdb_store::StoreStats;
use tumult_analytics::error::AnalyticsError;
use tumult_analytics::query_row::QueryRow;
use tumult_core::types::Journal;

use super::ClickHouseStore;

impl tumult_analytics::backend::private::Sealed for ClickHouseStore {}

// Synchronous wrapper for AnalyticsBackend trait.
//
// `tokio::task::block_in_place` is used instead of a bare
// `Handle::current().block_on(...)` because `block_on` panics (or deadlocks)
// when called from inside an already-running Tokio task.  `block_in_place`
// moves the calling thread out of the async worker pool first, making the
// nested `block_on` safe on a multi-threaded runtime.
impl AnalyticsBackend for ClickHouseStore {
    fn ingest_journal(&self, journal: &Journal) -> Result<bool, AnalyticsError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.ingest_journal_async(journal))
        })
    }

    fn query(&self, sql: &str) -> Result<Vec<QueryRow>, AnalyticsError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.query_async(sql))
        })
    }

    fn query_columns(&self, sql: &str) -> Result<Vec<String>, AnalyticsError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.query_columns_async(sql))
        })
    }

    fn experiment_count(&self) -> Result<usize, AnalyticsError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.experiment_count_async())
        })
    }

    fn stats(&self) -> Result<StoreStats, AnalyticsError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.stats_async())
        })
    }

    fn purge_older_than_days(&self, days: u32) -> Result<usize, AnalyticsError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.purge_older_than_days_async(days))
        })
    }

    fn schema_version(&self) -> Result<i64, AnalyticsError> {
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(self.schema_version_async())
        })
    }
}
