//! Synchronous `AnalyticsBackend` trait implementation for [`ClickHouseStore`].

use tumult_analytics::backend::{AnalyticsBackend, StoreStats};
use tumult_analytics::error::AnalyticsError;
use tumult_analytics::query_row::QueryRow;
use tumult_core::sync_bridge::sync_await;
use tumult_core::types::Journal;

use super::ClickHouseStore;

impl tumult_analytics::backend::private::Sealed for ClickHouseStore {}

// Synchronous wrapper for AnalyticsBackend trait; see
// `tumult_core::sync_bridge::sync_await` for the multi-thread-runtime caveat.
impl AnalyticsBackend for ClickHouseStore {
    fn ingest_journal(&self, journal: &Journal) -> Result<bool, AnalyticsError> {
        sync_await(self.ingest_journal_async(journal))
    }

    fn query(&self, sql: &str) -> Result<Vec<QueryRow>, AnalyticsError> {
        sync_await(self.query_async(sql))
    }

    fn query_columns(&self, sql: &str) -> Result<Vec<String>, AnalyticsError> {
        sync_await(self.query_columns_async(sql))
    }

    fn experiment_count(&self) -> Result<usize, AnalyticsError> {
        sync_await(self.experiment_count_async())
    }

    fn stats(&self) -> Result<StoreStats, AnalyticsError> {
        sync_await(self.stats_async())
    }

    fn purge_older_than_days(&self, days: u32) -> Result<usize, AnalyticsError> {
        sync_await(self.purge_older_than_days_async(days))
    }

    fn schema_version(&self) -> Result<i64, AnalyticsError> {
        sync_await(self.schema_version_async())
    }
}
