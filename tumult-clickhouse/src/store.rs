//! `ClickHouse` analytics store — implements `AnalyticsBackend`.
//!
//! Uses typed Row structs and parameterized queries to prevent SQL injection.
//! Writes experiment data to `ClickHouse` `MergeTree` tables in the `tumult`
//! database, alongside `SigNoz`'s `OTel` data for cross-correlation.

use clickhouse::Client;
use serde::{Deserialize, Serialize};
use tumult_analytics::backend::AnalyticsBackend;
use tumult_analytics::duckdb_store::StoreStats;
use tumult_analytics::error::AnalyticsError;
use tumult_analytics::telemetry;
use tumult_core::types::Journal;

use crate::config::ClickHouseConfig;

const SCHEMA_VERSION: i64 = 1;

/// Retry configuration for `ClickHouse` connection attempts.
pub struct RetryConfig {
    /// Maximum number of connection attempts.
    pub max_attempts: u32,
    /// Backoff durations between retries (one per retry).
    pub backoff_durations: Vec<std::time::Duration>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            backoff_durations: vec![
                std::time::Duration::from_secs(2),
                std::time::Duration::from_secs(4),
                std::time::Duration::from_secs(8),
            ],
        }
    }
}

// ── Typed rows for safe insert/select ───────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, clickhouse::Row)]
struct ExperimentRow {
    experiment_id: String,
    title: String,
    status: String,
    started_at_ns: i64,
    ended_at_ns: i64,
    duration_ms: u64,
    method_step_count: i64,
    rollback_count: i64,
    hypothesis_before_met: Option<u8>,
    hypothesis_after_met: Option<u8>,
    estimate_accuracy: Option<f64>,
    resilience_score: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, clickhouse::Row)]
struct ActivityRow {
    experiment_id: String,
    name: String,
    activity_type: String,
    status: String,
    started_at_ns: i64,
    duration_ms: u64,
    output: Option<String>,
    error: Option<String>,
    phase: String,
}

#[derive(Debug, Deserialize, clickhouse::Row)]
struct CountRow {
    count: u64,
}

#[derive(Debug, Deserialize, clickhouse::Row)]
struct ValueRow {
    value: String,
}

// ── Store ───────────────────────────────────────────────────

/// ClickHouse-backed analytics store.
pub struct ClickHouseStore {
    client: Client,
    database: String,
    query_timeout: std::time::Duration,
}

impl ClickHouseStore {
    /// Connect to `ClickHouse` with retry and exponential backoff.
    ///
    /// Attempts up to `retry_config.max_attempts` times, sleeping between
    /// failures with the configured backoff durations.
    ///
    /// # Errors
    ///
    /// Returns an error if all connection attempts fail.
    pub async fn connect_with_retry(
        config: &ClickHouseConfig,
        retry_config: &RetryConfig,
    ) -> Result<Self, AnalyticsError> {
        let mut last_err = None;
        for attempt in 0..retry_config.max_attempts {
            match Self::connect(config).await {
                Ok(store) => return Ok(store),
                Err(e) => {
                    let backoff = retry_config
                        .backoff_durations
                        .get(attempt as usize)
                        .copied()
                        .unwrap_or(std::time::Duration::from_secs(8));
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_attempts = retry_config.max_attempts,
                        backoff_s = backoff.as_secs(),
                        error = %e,
                        "ClickHouse connection failed, retrying"
                    );
                    last_err = Some(e);
                    tokio::time::sleep(backoff).await;
                }
            }
        }
        Err(last_err.unwrap_or_else(|| {
            AnalyticsError::ClickHouse("connection failed after all retries".into())
        }))
    }

    /// Connect to `ClickHouse` and initialize the schema.
    ///
    /// # Errors
    ///
    /// Returns an error if the `ClickHouse` connection or schema initialisation fails.
    pub async fn connect(config: &ClickHouseConfig) -> Result<Self, AnalyticsError> {
        let _span = crate::telemetry::begin_connect(&config.url, &config.database);

        let client = Client::default()
            .with_url(&config.url)
            .with_user(&config.user)
            .with_password(&config.password)
            .with_database(&config.database);

        let store = Self {
            client,
            database: config.database.clone(),
            query_timeout: config.query_timeout,
        };

        store.init_schema().await?;
        crate::telemetry::event_schema_initialized(&config.database, SCHEMA_VERSION);
        Ok(store)
    }

    async fn init_schema(&self) -> Result<(), AnalyticsError> {
        // Validate database name (alphanumeric + underscore only)
        if !self
            .database
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(AnalyticsError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("invalid database name: {}", self.database),
            )));
        }

        self.execute_ddl(&format!(
            "CREATE DATABASE IF NOT EXISTS `{}`",
            self.database
        ))
        .await?;

        self.execute_ddl(
            "CREATE TABLE IF NOT EXISTS experiments (
                experiment_id String,
                title String,
                status String,
                started_at_ns Int64,
                ended_at_ns Int64,
                duration_ms UInt64,
                method_step_count Int64,
                rollback_count Int64,
                hypothesis_before_met Nullable(UInt8),
                hypothesis_after_met Nullable(UInt8),
                estimate_accuracy Nullable(Float64),
                resilience_score Nullable(Float64)
            ) ENGINE = ReplacingMergeTree()
            ORDER BY (experiment_id)
            PRIMARY KEY (experiment_id)",
        )
        .await?;

        self.execute_ddl(
            "CREATE TABLE IF NOT EXISTS activity_results (
                experiment_id String,
                name String,
                activity_type String,
                status String,
                started_at_ns Int64,
                duration_ms UInt64,
                output Nullable(String),
                error Nullable(String),
                phase String
            ) ENGINE = MergeTree()
            ORDER BY (experiment_id, started_at_ns)",
        )
        .await?;

        self.execute_ddl(
            "CREATE TABLE IF NOT EXISTS schema_meta (
                key String,
                value String
            ) ENGINE = ReplacingMergeTree()
            ORDER BY (key)",
        )
        .await?;

        // Insert schema version (ReplacingMergeTree handles dedup)
        self.execute_ddl(&format!(
            "INSERT INTO schema_meta (key, value) VALUES ('version', '{SCHEMA_VERSION}')"
        ))
        .await?;

        Ok(())
    }

    fn ch_err(e: &clickhouse::error::Error) -> AnalyticsError {
        AnalyticsError::ClickHouse(e.to_string())
    }

    /// Wrap an async operation with the configured query timeout.
    async fn with_timeout<T, F>(&self, fut: F) -> Result<T, AnalyticsError>
    where
        F: std::future::Future<Output = Result<T, AnalyticsError>>,
    {
        tokio::time::timeout(self.query_timeout, fut)
            .await
            .map_err(|_| {
                AnalyticsError::ClickHouse(format!(
                    "query timed out after {:?}",
                    self.query_timeout
                ))
            })?
    }

    async fn execute_ddl(&self, sql: &str) -> Result<(), AnalyticsError> {
        crate::telemetry::event_ddl_executed(sql);
        self.with_timeout(async {
            self.client
                .query(sql)
                .execute()
                .await
                .map_err(|e| Self::ch_err(&e))
        })
        .await
    }

    /// Async ingest using typed Row inserts (no SQL interpolation).
    ///
    /// # Errors
    ///
    /// Returns an error if the `ClickHouse` insert or duplicate check fails.
    pub async fn ingest_journal_async(&self, journal: &Journal) -> Result<bool, AnalyticsError> {
        let _span = telemetry::begin_ingest(&journal.experiment_id, &journal.experiment_title);

        // Check duplicate via parameterized bind
        let count = self
            .with_timeout(async {
                self.client
                    .query("SELECT count() as count FROM experiments WHERE experiment_id = ?")
                    .bind(&journal.experiment_id)
                    .fetch_one::<CountRow>()
                    .await
                    .map_err(|e| Self::ch_err(&e))
            })
            .await?;

        if count.count > 0 {
            telemetry::event_journal_duplicate(&journal.experiment_id);
            return Ok(false);
        }

        // Type-safe insert for experiment
        let exp_row = ExperimentRow {
            experiment_id: journal.experiment_id.clone(),
            title: journal.experiment_title.clone(),
            status: journal.status.to_string(),
            started_at_ns: journal.started_at_ns,
            ended_at_ns: journal.ended_at_ns,
            duration_ms: journal.duration_ms,
            // usize → i64: result counts in chaos experiments are always << i64::MAX.
            #[allow(clippy::cast_possible_wrap)]
            method_step_count: journal.method_results.len() as i64,
            // usize → i64: result counts in chaos experiments are always << i64::MAX.
            #[allow(clippy::cast_possible_wrap)]
            rollback_count: journal.rollback_results.len() as i64,
            hypothesis_before_met: journal
                .steady_state_before
                .as_ref()
                .map(|h| u8::from(h.met)),
            hypothesis_after_met: journal.steady_state_after.as_ref().map(|h| u8::from(h.met)),
            estimate_accuracy: journal.analysis.as_ref().and_then(|a| a.estimate_accuracy),
            resilience_score: journal.analysis.as_ref().and_then(|a| a.resilience_score),
        };

        let mut insert = self
            .client
            .insert::<ExperimentRow>("experiments")
            .await
            .map_err(|e| Self::ch_err(&e))?;
        insert.write(&exp_row).await.map_err(|e| Self::ch_err(&e))?;
        insert.end().await.map_err(|e| Self::ch_err(&e))?;

        // Type-safe insert for activity results
        let mut activity_count = 0usize;
        let phases: Vec<(&str, &[tumult_core::types::ActivityResult])> = vec![
            (
                "hypothesis_before",
                journal
                    .steady_state_before
                    .as_ref()
                    .map_or(&[], |h| h.probe_results.as_slice()),
            ),
            ("method", &journal.method_results),
            (
                "hypothesis_after",
                journal
                    .steady_state_after
                    .as_ref()
                    .map_or(&[], |h| h.probe_results.as_slice()),
            ),
            ("rollback", &journal.rollback_results),
        ];

        let mut act_insert = self
            .client
            .insert::<ActivityRow>("activity_results")
            .await
            .map_err(|e| Self::ch_err(&e))?;

        for (phase, results) in phases {
            for r in results {
                let row = ActivityRow {
                    experiment_id: journal.experiment_id.clone(),
                    name: r.name.clone(),
                    activity_type: r.activity_type.to_string(),
                    status: r.status.to_string(),
                    started_at_ns: r.started_at_ns,
                    duration_ms: r.duration_ms,
                    output: r.output.clone(),
                    error: r.error.clone(),
                    phase: phase.to_string(),
                };
                act_insert.write(&row).await.map_err(|e| Self::ch_err(&e))?;
                activity_count += 1;
            }
        }

        act_insert.end().await.map_err(|e| Self::ch_err(&e))?;

        telemetry::event_journal_ingested(&journal.experiment_id, activity_count);
        crate::telemetry::record_store_gauges(0, 0); // will be updated on next stats call
        Ok(true)
    }

    /// Async query execution — returns rows as TSV-parsed string vectors.
    pub(crate) async fn query_async(&self, sql: &str) -> Result<Vec<Vec<String>>, AnalyticsError> {
        let _span = telemetry::begin_query(sql);

        let mut cursor = self
            .client
            .query(sql)
            .fetch_bytes("TabSeparated")
            .map_err(|e| Self::ch_err(&e))?;

        let mut result = Vec::new();
        while let Some(bytes) = cursor.next().await.map_err(|e| Self::ch_err(&e))? {
            let line = String::from_utf8_lossy(&bytes);
            let fields: Vec<String> = line
                .split('\t')
                .map(std::string::ToString::to_string)
                .collect();
            result.push(fields);
        }

        telemetry::event_query_executed(result.len(), 0);
        Ok(result)
    }

    pub(crate) async fn query_columns_async(
        &self,
        sql: &str,
    ) -> Result<Vec<String>, AnalyticsError> {
        let mut cursor = self
            .client
            .query(sql)
            .fetch_bytes("TabSeparatedWithNames")
            .map_err(|e| Self::ch_err(&e))?;

        // First row is header with column names
        if let Some(bytes) = cursor.next().await.map_err(|e| Self::ch_err(&e))? {
            let line = String::from_utf8_lossy(&bytes);
            return Ok(line
                .split('\t')
                .map(std::string::ToString::to_string)
                .collect());
        }
        Ok(vec![])
    }

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
    /// # Panics
    ///
    /// Panics if the retention period in nanoseconds overflows `i64` (requires
    /// `days > 106_751` — approximately 292 years).
    ///
    /// # Errors
    ///
    /// Returns an error if any `ClickHouse` query or delete operation fails.
    pub async fn purge_older_than_days_async(&self, days: u32) -> Result<usize, AnalyticsError> {
        let _span = telemetry::begin_purge(days);

        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX);
        let retention_ns = i64::from(days)
            .checked_mul(86_400_000_000_000)
            .expect("retention period overflow");
        let cutoff_ns = now_ns.saturating_sub(retention_ns);

        let before = self.experiment_count_async().await?;

        // Parameterized delete via bind
        self.client
            .query(
                "ALTER TABLE activity_results DELETE WHERE experiment_id IN \
                 (SELECT experiment_id FROM experiments WHERE started_at_ns < ?)",
            )
            .bind(cutoff_ns)
            .execute()
            .await
            .map_err(|e| Self::ch_err(&e))?;

        self.client
            .query("ALTER TABLE experiments DELETE WHERE started_at_ns < ?")
            .bind(cutoff_ns)
            .execute()
            .await
            .map_err(|e| Self::ch_err(&e))?;

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

impl tumult_analytics::backend::private::Sealed for ClickHouseStore {}

// Synchronous wrapper for AnalyticsBackend trait.
impl AnalyticsBackend for ClickHouseStore {
    fn ingest_journal(&self, journal: &Journal) -> Result<bool, AnalyticsError> {
        tokio::runtime::Handle::current().block_on(self.ingest_journal_async(journal))
    }

    fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, AnalyticsError> {
        tokio::runtime::Handle::current().block_on(self.query_async(sql))
    }

    fn query_columns(&self, sql: &str) -> Result<Vec<String>, AnalyticsError> {
        tokio::runtime::Handle::current().block_on(self.query_columns_async(sql))
    }

    fn experiment_count(&self) -> Result<usize, AnalyticsError> {
        tokio::runtime::Handle::current().block_on(self.experiment_count_async())
    }

    fn stats(&self) -> Result<StoreStats, AnalyticsError> {
        tokio::runtime::Handle::current().block_on(self.stats_async())
    }

    fn purge_older_than_days(&self, days: u32) -> Result<usize, AnalyticsError> {
        tokio::runtime::Handle::current().block_on(self.purge_older_than_days_async(days))
    }

    fn schema_version(&self) -> Result<i64, AnalyticsError> {
        tokio::runtime::Handle::current().block_on(self.schema_version_async())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_config_has_correct_defaults() {
        let config = RetryConfig::default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.backoff_durations.len(), 3);
        assert_eq!(
            config.backoff_durations[0],
            std::time::Duration::from_secs(2)
        );
        assert_eq!(
            config.backoff_durations[1],
            std::time::Duration::from_secs(4)
        );
        assert_eq!(
            config.backoff_durations[2],
            std::time::Duration::from_secs(8)
        );
    }

    #[test]
    fn config_creates_valid_client() {
        let config = ClickHouseConfig::default();
        let _client = Client::default()
            .with_url(&config.url)
            .with_user(&config.user)
            .with_password(&config.password)
            .with_database(&config.database);
    }

    #[test]
    fn schema_version_constant_is_valid() {
        const _: () = assert!(SCHEMA_VERSION >= 1);
    }

    #[test]
    fn experiment_row_serializable() {
        let row = ExperimentRow {
            experiment_id: "e-001".into(),
            title: "test".into(),
            status: "Completed".into(),
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_060_000_000_000,
            duration_ms: 60_000,
            method_step_count: 1,
            rollback_count: 0,
            hypothesis_before_met: Some(1),
            hypothesis_after_met: None,
            estimate_accuracy: Some(0.95),
            resilience_score: None,
        };
        // Verify serde serialization works
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("e-001"));
    }

    #[test]
    fn activity_row_serializable() {
        let row = ActivityRow {
            experiment_id: "e-001".into(),
            name: "test-action".into(),
            activity_type: "Action".into(),
            status: "Succeeded".into(),
            started_at_ns: 1_774_980_000_000_000_000,
            duration_ms: 500,
            output: Some("ok".into()),
            error: None,
            phase: "method".into(),
        };
        let json = serde_json::to_string(&row).unwrap();
        assert!(json.contains("test-action"));
    }
}
