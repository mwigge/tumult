//! Aggregate statistics, retention purge, and schema-version queries.

use tumult_lake::backend::StoreStats;
use tumult_lake::error::AnalyticsError;
use tumult_lake::telemetry;

use super::rows::{CountRow, ValueRow};
use super::ClickHouseStore;

impl ClickHouseStore {
    /// Returns the total number of experiments stored in `ClickHouse`.
    ///
    /// # Errors
    ///
    /// Returns an error if the `ClickHouse` query fails or times out.
    pub async fn experiment_count_async(&self) -> Result<usize, AnalyticsError> {
        let row = self
            .with_timeout(async {
                self.client
                    .query("SELECT count() as count FROM experiments")
                    .fetch_one::<CountRow>()
                    .await
                    .map_err(|e| Self::ch_err(&e))
            })
            .await?;
        // u64 → usize: row counts from ClickHouse are always within usize range on
        // any supported 64-bit target; truncation on hypothetical 32-bit targets is
        // acceptable for a count that drives display logic only.
        #[allow(clippy::cast_possible_truncation)]
        Ok(row.count as usize)
    }

    /// Returns aggregate store statistics (experiment and activity counts).
    ///
    /// # Errors
    ///
    /// Returns an error if either `ClickHouse` query fails or times out.
    pub async fn stats_async(&self) -> Result<StoreStats, AnalyticsError> {
        let exp = self.experiment_count_async().await?;
        let act_row = self
            .with_timeout(async {
                self.client
                    .query("SELECT count() as count FROM activity_results")
                    .fetch_one::<CountRow>()
                    .await
                    .map_err(|e| Self::ch_err(&e))
            })
            .await?;
        // u64 → usize: same rationale as experiment_count_async above.
        #[allow(clippy::cast_possible_truncation)]
        let activity_count = act_row.count as usize;
        Ok(StoreStats {
            experiment_count: exp,
            activity_count,
        })
    }

    /// Deletes experiments (and their activity results) older than `days` days.
    ///
    /// `ALTER TABLE ... DELETE` mutations in `ClickHouse` are asynchronous by
    /// default, so a count taken immediately after issuing them would report 0
    /// rows removed even when the purge later succeeds. Both statements are
    /// therefore issued with `SETTINGS mutations_sync = 1`, which blocks until
    /// the mutation has been applied on this replica — the `before − after`
    /// count below then reflects the rows actually deleted rather than a
    /// fabrication.
    ///
    /// # Errors
    ///
    /// Returns [`AnalyticsError::Internal`] if the retention period overflows
    /// `i64` nanoseconds (requires `days > 106_751`), or an error if any
    /// `ClickHouse` query or delete operation fails or times out (the
    /// configured query timeout also bounds the synchronous mutation wait).
    pub async fn purge_older_than_days_async(&self, days: u32) -> Result<usize, AnalyticsError> {
        let _span = telemetry::begin_purge(days);

        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX);
        let retention_ns = i64::from(days)
            .checked_mul(86_400_000_000_000)
            .ok_or_else(|| {
                AnalyticsError::Internal(format!(
                    "retention period of {days} days overflows i64 nanoseconds"
                ))
            })?;
        let cutoff_ns = now_ns.saturating_sub(retention_ns);

        let before = self.experiment_count_async().await?;

        // Parameterized delete via bind; `mutations_sync = 1` makes the ALTER
        // return only after the mutation is applied (see the doc comment).
        self.with_timeout(async {
            self.client
                .query(
                    "ALTER TABLE activity_results DELETE WHERE experiment_id IN \
                     (SELECT experiment_id FROM experiments WHERE started_at_ns < ?) \
                     SETTINGS mutations_sync = 1",
                )
                .bind(cutoff_ns)
                .execute()
                .await
                .map_err(|e| Self::ch_err(&e))
        })
        .await?;

        self.with_timeout(async {
            self.client
                .query(
                    "ALTER TABLE experiments DELETE WHERE started_at_ns < ? \
                     SETTINGS mutations_sync = 1",
                )
                .bind(cutoff_ns)
                .execute()
                .await
                .map_err(|e| Self::ch_err(&e))
        })
        .await?;

        let after = self.experiment_count_async().await?;
        let purged = before.saturating_sub(after);
        telemetry::event_purge_completed(purged, after);
        Ok(purged)
    }

    /// Returns the schema version stored in the `schema_meta` table.
    ///
    /// # Errors
    ///
    /// Returns an error if the `ClickHouse` query fails or the stored value is not
    /// a valid `i64`.
    pub async fn schema_version_async(&self) -> Result<i64, AnalyticsError> {
        let row = self
            .with_timeout(async {
                self.client
                    .query("SELECT value FROM schema_meta WHERE key = 'version' LIMIT 1")
                    .fetch_one::<ValueRow>()
                    .await
                    .map_err(|e| Self::ch_err(&e))
            })
            .await?;
        row.value.parse::<i64>().map_err(|_| {
            AnalyticsError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid schema version: {}", row.value),
            ))
        })
    }
}
