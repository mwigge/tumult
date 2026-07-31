//! `ClickHouse` analytics store — implements `AnalyticsBackend`.
//!
//! Uses typed Row structs and parameterized queries to prevent SQL injection.
//! Writes experiment data to `ClickHouse` `MergeTree` tables in the `tumult`
//! database, alongside `SigNoz`'s `OTel` data for cross-correlation.

use clickhouse::Client;
use tumult_lake::error::AnalyticsError;

use crate::config::ClickHouseConfig;

mod backend;
mod ingest;
mod queries;
mod rows;

#[cfg(test)]
mod tests;

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
}
