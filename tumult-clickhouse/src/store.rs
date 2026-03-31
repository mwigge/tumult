//! ClickHouse analytics store — implements AnalyticsBackend.
//!
//! Writes experiment data to ClickHouse MergeTree tables in the `tumult`
//! database. When running alongside SigNoz, experiment data lives in the
//! same ClickHouse instance as OTel traces/metrics/logs, enabling
//! cross-correlation queries.

use clickhouse_client::Client;
use tumult_analytics::backend::AnalyticsBackend;
use tumult_analytics::duckdb_store::StoreStats;
use tumult_analytics::error::AnalyticsError;
use tumult_analytics::telemetry;
use tumult_core::types::Journal;

use crate::config::ClickHouseConfig;

const SCHEMA_VERSION: i64 = 1;

/// ClickHouse-backed analytics store.
///
/// Connects to an external ClickHouse instance and creates the `tumult`
/// database with `experiments` and `activity_results` tables.
pub struct ClickHouseStore {
    client: Client,
    database: String,
}

impl ClickHouseStore {
    /// Connect to ClickHouse and initialize the schema.
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
        };

        store.init_schema().await?;
        crate::telemetry::event_schema_initialized(&config.database, SCHEMA_VERSION);
        crate::telemetry::record_store_gauges(0, 0);
        Ok(store)
    }

    async fn init_schema(&self) -> Result<(), AnalyticsError> {
        // Create database if not exists
        let create_db = format!("CREATE DATABASE IF NOT EXISTS {}", self.database);
        self.execute_ddl(&create_db).await?;

        // Experiments table — MergeTree ordered by timestamp
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

        // Activity results table — MergeTree ordered by experiment + timestamp
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

        // Schema version tracking
        self.execute_ddl(
            "CREATE TABLE IF NOT EXISTS schema_meta (
                key String,
                value String
            ) ENGINE = ReplacingMergeTree()
            ORDER BY (key)",
        )
        .await?;

        // Insert schema version if not present
        self.execute_ddl(&format!(
            "INSERT INTO schema_meta (key, value) \
             SELECT 'version', '{}' \
             WHERE NOT EXISTS (SELECT 1 FROM schema_meta WHERE key = 'version')",
            SCHEMA_VERSION
        ))
        .await?;

        Ok(())
    }

    async fn execute_ddl(&self, sql: &str) -> Result<(), AnalyticsError> {
        self.client
            .query(sql)
            .execute()
            .await
            .map_err(|e| AnalyticsError::Io(std::io::Error::other(e.to_string())))
    }

    async fn query_one_string(&self, sql: &str) -> Result<String, AnalyticsError> {
        let row = self
            .client
            .query(sql)
            .fetch_one::<String>()
            .await
            .map_err(|e| AnalyticsError::Io(std::io::Error::other(e.to_string())))?;
        Ok(row)
    }

    /// Async ingest — preferred when called from async context.
    pub async fn ingest_journal_async(&self, journal: &Journal) -> Result<bool, AnalyticsError> {
        let _span = telemetry::begin_ingest(&journal.experiment_id, &journal.experiment_title);

        // Check for duplicate
        let count_sql = format!(
            "SELECT count() FROM experiments WHERE experiment_id = '{}'",
            journal.experiment_id.replace('\'', "\\'")
        );
        let count = self.query_one_string(&count_sql).await?;
        if count != "0" {
            telemetry::event_journal_duplicate(&journal.experiment_id);
            return Ok(false);
        }

        // Insert experiment row
        let hyp_before = journal
            .steady_state_before
            .as_ref()
            .map(|h| if h.met { 1u8 } else { 0u8 });
        let hyp_after = journal
            .steady_state_after
            .as_ref()
            .map(|h| if h.met { 1u8 } else { 0u8 });
        let est_acc = journal.analysis.as_ref().and_then(|a| a.estimate_accuracy);
        let res_score = journal.analysis.as_ref().and_then(|a| a.resilience_score);

        let insert_exp = format!(
            "INSERT INTO experiments VALUES ('{}', '{}', '{:?}', {}, {}, {}, {}, {}, {}, {}, {}, {})",
            journal.experiment_id.replace('\'', "\\'"),
            journal.experiment_title.replace('\'', "\\'"),
            journal.status,
            journal.started_at_ns,
            journal.ended_at_ns,
            journal.duration_ms,
            journal.method_results.len(),
            journal.rollback_results.len(),
            hyp_before.map_or("NULL".to_string(), |v| v.to_string()),
            hyp_after.map_or("NULL".to_string(), |v| v.to_string()),
            est_acc.map_or("NULL".to_string(), |v| format!("{:.6}", v)),
            res_score.map_or("NULL".to_string(), |v| format!("{:.6}", v)),
        );
        self.execute_ddl(&insert_exp).await?;

        // Insert activity results
        let mut activity_count = 0;
        let phases: Vec<(&str, &[tumult_core::types::ActivityResult])> = vec![
            (
                "hypothesis_before",
                journal
                    .steady_state_before
                    .as_ref()
                    .map(|h| h.probe_results.as_slice())
                    .unwrap_or(&[]),
            ),
            ("method", &journal.method_results),
            (
                "hypothesis_after",
                journal
                    .steady_state_after
                    .as_ref()
                    .map(|h| h.probe_results.as_slice())
                    .unwrap_or(&[]),
            ),
            ("rollback", &journal.rollback_results),
        ];

        for (phase, results) in phases {
            for r in results {
                let insert_act = format!(
                    "INSERT INTO activity_results VALUES ('{}', '{}', '{:?}', '{:?}', {}, {}, {}, {}, '{}')",
                    journal.experiment_id.replace('\'', "\\'"),
                    r.name.replace('\'', "\\'"),
                    r.activity_type,
                    r.status,
                    r.started_at_ns,
                    r.duration_ms,
                    r.output.as_ref().map_or("NULL".to_string(), |v| format!("'{}'", v.replace('\'', "\\'"))),
                    r.error.as_ref().map_or("NULL".to_string(), |v| format!("'{}'", v.replace('\'', "\\'"))),
                    phase,
                );
                self.execute_ddl(&insert_act).await?;
                activity_count += 1;
            }
        }

        telemetry::event_journal_ingested(&journal.experiment_id, activity_count);

        // Update gauges after ingestion
        if let Ok(stats) = self.stats_async().await {
            crate::telemetry::record_store_gauges(stats.experiment_count, stats.activity_count);
        }

        Ok(true)
    }

    /// Async query execution.
    pub async fn query_async(&self, sql: &str) -> Result<Vec<Vec<String>>, AnalyticsError> {
        let _span = telemetry::begin_query(sql);

        // ClickHouse returns TSV by default via HTTP
        let raw = self
            .client
            .query(sql)
            .fetch_all::<Vec<String>>()
            .await
            .map_err(|e| AnalyticsError::Io(std::io::Error::other(e.to_string())))?;

        telemetry::event_query_executed(raw.len(), 0);
        Ok(raw)
    }

    /// Async experiment count.
    pub async fn experiment_count_async(&self) -> Result<usize, AnalyticsError> {
        let count = self
            .query_one_string("SELECT count() FROM experiments")
            .await?;
        Ok(count.parse::<usize>().unwrap_or(0))
    }

    /// Async stats.
    pub async fn stats_async(&self) -> Result<StoreStats, AnalyticsError> {
        let exp = self.experiment_count_async().await?;
        let act = self
            .query_one_string("SELECT count() FROM activity_results")
            .await?;
        Ok(StoreStats {
            experiment_count: exp,
            activity_count: act.parse::<usize>().unwrap_or(0),
        })
    }

    /// Async purge.
    pub async fn purge_older_than_days_async(&self, days: u32) -> Result<usize, AnalyticsError> {
        let _span = telemetry::begin_purge(days);

        let now_ns = chrono::Utc::now()
            .timestamp_nanos_opt()
            .expect("system time before year 2262");
        let retention_ns = i64::from(days)
            .checked_mul(86_400_000_000_000)
            .expect("retention period overflow");
        let cutoff_ns = now_ns.saturating_sub(retention_ns);

        // Count before delete (ClickHouse doesn't return affected rows easily)
        let before = self.experiment_count_async().await?;

        self.execute_ddl(&format!(
            "ALTER TABLE activity_results DELETE WHERE experiment_id IN \
             (SELECT experiment_id FROM experiments WHERE started_at_ns < {})",
            cutoff_ns
        ))
        .await?;

        self.execute_ddl(&format!(
            "ALTER TABLE experiments DELETE WHERE started_at_ns < {}",
            cutoff_ns
        ))
        .await?;

        let after = self.experiment_count_async().await?;
        let purged = before.saturating_sub(after);
        telemetry::event_purge_completed(purged, after);
        Ok(purged)
    }

    /// Async schema version.
    pub async fn schema_version_async(&self) -> Result<i64, AnalyticsError> {
        let version = self
            .query_one_string("SELECT value FROM schema_meta WHERE key = 'version' LIMIT 1")
            .await?;
        version.parse::<i64>().map_err(|_| {
            AnalyticsError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid schema version: {}", version),
            ))
        })
    }
}

// Synchronous wrapper for AnalyticsBackend trait (uses tokio block_on).
impl AnalyticsBackend for ClickHouseStore {
    fn ingest_journal(&self, journal: &Journal) -> Result<bool, AnalyticsError> {
        tokio::runtime::Handle::current().block_on(self.ingest_journal_async(journal))
    }

    fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, AnalyticsError> {
        tokio::runtime::Handle::current().block_on(self.query_async(sql))
    }

    fn query_columns(&self, sql: &str) -> Result<Vec<String>, AnalyticsError> {
        // ClickHouse: query with LIMIT 0 to get column names
        let desc_sql = format!("DESCRIBE ({})", sql);
        let rows = tokio::runtime::Handle::current().block_on(self.query_async(&desc_sql))?;
        Ok(rows
            .into_iter()
            .map(|r| r.first().cloned().unwrap_or_default())
            .collect())
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
    fn config_creates_valid_client() {
        let config = ClickHouseConfig::default();
        // Just verify construction doesn't panic
        let _client = Client::default()
            .with_url(&config.url)
            .with_user(&config.user)
            .with_password(&config.password)
            .with_database(&config.database);
    }

    #[test]
    fn schema_version_constant_is_valid() {
        assert!(SCHEMA_VERSION >= 1);
    }

    // Integration tests (require running ClickHouse) are in tests/integration.rs
    // and gated behind #[ignore] — run with: cargo test --ignored
}
