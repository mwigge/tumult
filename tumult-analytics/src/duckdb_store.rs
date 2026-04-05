//! `DuckDB` embedded analytics store.
//!
//! Provides both in-memory and persistent (file-backed) analytics stores.
//! Persistent stores use WAL mode for crash safety, deduplicate journals
//! by `experiment_id`, and support schema versioning for future migrations.
//!
//! **Thread safety:** `AnalyticsStore` wraps a single `DuckDB` `Connection` and
//! is NOT thread-safe. For shared access, wrap in `Arc<Mutex<AnalyticsStore>>`.
//!
//! **Encryption limitation:** `DuckDB` does not support transparent
//! encryption-at-rest. The database file is stored in plaintext on disk.
//! Protect sensitive experiment data by relying on filesystem-level encryption
//! (e.g. LUKS, `FileVault`, `BitLocker`) and by restricting the store directory
//! permissions to `0o700` (which [`AnalyticsStore::open`] applies automatically).

use std::path::{Path, PathBuf};

use arrow::record_batch::RecordBatch;
use duckdb::{params, Connection};
use tumult_core::types::Journal;

use crate::arrow_convert::{
    journal_to_activity_batch, journal_to_experiment_batch, journal_to_load_batch,
};
use crate::error::AnalyticsError;
use crate::export::{export_parquet, import_parquet};
use crate::query_row::QueryRow;
use crate::telemetry;

const CURRENT_SCHEMA_VERSION: i64 = 1;

pub struct StoreStats {
    pub experiment_count: usize,
    pub activity_count: usize,
}

/// Embedded `DuckDB` analytics store for experiment journals.
///
/// **Not thread-safe.** Each instance holds a single `DuckDB` connection.
/// For concurrent access, wrap in `Arc<Mutex<AnalyticsStore>>`.
///
/// # Security
///
/// `DuckDB` does not encrypt data at rest by default. The database file at
/// `~/.tumult/analytics.duckdb` is stored in plaintext on disk. For
/// environments where experiment data is sensitive, place the store on an
/// encrypted volume:
///
/// - **Linux**: LUKS full-disk or directory encryption (`fscrypt`, `ecryptfs`)
/// - **macOS**: `FileVault 2` (whole-disk) or an encrypted APFS volume
/// - **Windows**: `BitLocker` or an encrypted home directory
///
/// The store directory is automatically created with mode `0o700` (owner
/// read/write/execute only) by [`AnalyticsStore::open`], limiting access to
/// the process owner. However, directory permissions are not a substitute
/// for encryption — a privileged user or physical attacker can still access
/// the file without encryption.
///
/// Use the `TUMULT_STORE_PATH` environment variable to redirect the persistent
/// store to a path on an encrypted volume when the default location is not
/// suitable.
pub struct AnalyticsStore {
    conn: Connection,
}

impl AnalyticsStore {
    /// Returns the default persistent store path: `~/.tumult/analytics.duckdb`
    ///
    /// # Panics
    ///
    /// Panics if the home directory cannot be determined.
    #[must_use]
    pub fn default_path() -> PathBuf {
        let home = dirs_next::home_dir().expect("cannot determine home directory");
        home.join(".tumult").join("analytics.duckdb")
    }

    /// # Errors
    ///
    /// Returns an error if the in-memory `DuckDB` connection or schema initialisation fails.
    #[must_use = "callers must handle connection or schema errors"]
    pub fn in_memory() -> Result<Self, AnalyticsError> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    /// # Errors
    ///
    /// Returns an error if the `DuckDB` file cannot be opened or schema initialisation fails.
    #[must_use = "callers must handle file open or schema errors"]
    pub fn open(path: &Path) -> Result<Self, AnalyticsError> {
        // Ensure parent directory exists with restricted permissions
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&self) -> Result<(), AnalyticsError> {
        self.create_tables()?;
        self.ensure_schema_version()?;
        Ok(())
    }

    fn create_tables(&self) -> Result<(), AnalyticsError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS experiments (
                experiment_id VARCHAR NOT NULL, title VARCHAR NOT NULL,
                status VARCHAR NOT NULL, started_at_ns BIGINT NOT NULL,
                ended_at_ns BIGINT NOT NULL, duration_ms UBIGINT NOT NULL,
                method_step_count BIGINT NOT NULL, rollback_count BIGINT NOT NULL,
                hypothesis_before_met BOOLEAN, hypothesis_after_met BOOLEAN,
                estimate_accuracy DOUBLE, resilience_score DOUBLE
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_experiments_id
                ON experiments (experiment_id);
            CREATE TABLE IF NOT EXISTS activity_results (
                experiment_id VARCHAR NOT NULL, name VARCHAR NOT NULL,
                activity_type VARCHAR NOT NULL, status VARCHAR NOT NULL,
                started_at_ns BIGINT NOT NULL, duration_ms UBIGINT NOT NULL,
                output VARCHAR, error VARCHAR, phase VARCHAR NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_activities_experiment_id
                ON activity_results (experiment_id);
            CREATE TABLE IF NOT EXISTS load_results (
                experiment_id VARCHAR NOT NULL, tool VARCHAR NOT NULL,
                started_at_ns BIGINT NOT NULL, ended_at_ns BIGINT NOT NULL,
                duration_s DOUBLE NOT NULL, vus INTEGER NOT NULL,
                throughput_rps DOUBLE NOT NULL, latency_p50_ms DOUBLE NOT NULL,
                latency_p95_ms DOUBLE NOT NULL, latency_p99_ms DOUBLE NOT NULL,
                error_rate DOUBLE NOT NULL, total_requests UBIGINT NOT NULL,
                thresholds_met BOOLEAN NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_load_experiment_id
                ON load_results (experiment_id);
            CREATE TABLE IF NOT EXISTS schema_meta (
                key VARCHAR PRIMARY KEY, value BIGINT NOT NULL
            );",
        )?;
        Ok(())
    }

    fn ensure_schema_version(&self) -> Result<(), AnalyticsError> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM schema_meta WHERE key = 'version'")?;
        // Read as i64 directly — the column is now BIGINT, no String round-trip.
        let version: Option<i64> = stmt.query_row(params![], |row| row.get(0)).ok();

        if version.is_none() {
            self.conn.execute(
                "INSERT INTO schema_meta (key, value) VALUES ('version', ?)",
                // Bind i64 directly — avoids a String allocation and type mismatch.
                params![CURRENT_SCHEMA_VERSION],
            )?;
        }
        // Future: if version < CURRENT_SCHEMA_VERSION, run migrations here
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error if the schema version cannot be read.
    #[must_use = "callers must use the returned schema version"]
    pub fn schema_version(&self) -> Result<i64, AnalyticsError> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM schema_meta WHERE key = 'version'")?;
        // Column is BIGINT — no String parse round-trip needed.
        stmt.query_row(params![], |row| row.get(0))
            .map_err(AnalyticsError::from)
    }

    /// Check if an `experiment_id` already exists in the store.
    fn experiment_exists(&self, experiment_id: &str) -> Result<bool, AnalyticsError> {
        let mut stmt = self
            .conn
            .prepare("SELECT count(*) FROM experiments WHERE experiment_id = ?")?;
        let count: i64 = stmt.query_row(params![experiment_id], |row| row.get(0))?;
        Ok(count > 0)
    }

    /// Ingest a single experiment journal into the analytics store.
    /// Skips ingestion if the `experiment_id` already exists (incremental/dedup).
    ///
    /// Returns true if the journal was ingested, false if it was a duplicate.
    ///
    /// # Errors
    ///
    /// Returns an error if the `DuckDB` insert or Arrow conversion fails.
    ///
    /// # Examples
    ///
    /// ```
    /// use tumult_analytics::AnalyticsStore;
    /// use tumult_core::types::*;
    ///
    /// let store = AnalyticsStore::in_memory().unwrap();
    ///
    /// let journal = Journal {
    ///     experiment_title: "demo".into(),
    ///     experiment_id: "e-001".into(),
    ///     status: ExperimentStatus::Completed,
    ///     started_at_ns: 1_700_000_000_000_000_000,
    ///     ended_at_ns: 1_700_000_060_000_000_000,
    ///     duration_ms: 60_000,
    ///     steady_state_before: None,
    ///     steady_state_after: None,
    ///     method_results: vec![],
    ///     rollback_results: vec![],
    ///     estimate: None,
    ///     baseline_result: None,
    ///     during_result: None,
    ///     post_result: None,
    ///     load_result: None,
    ///     analysis: None,
    ///     regulatory: None,
    ///     rollback_failures: 0,
    /// };
    ///
    /// store.ingest_journal(&journal).unwrap();
    /// assert_eq!(store.experiment_count().unwrap(), 1);
    /// ```
    #[must_use = "callers must check whether the journal was ingested or skipped as a duplicate"]
    pub fn ingest_journal(&self, journal: &Journal) -> Result<bool, AnalyticsError> {
        let _span = telemetry::begin_ingest(&journal.experiment_id, &journal.experiment_title);

        if self.experiment_exists(&journal.experiment_id)? {
            telemetry::event_journal_duplicate(&journal.experiment_id);
            return Ok(false);
        }
        let exp_batch = journal_to_experiment_batch(journal)?;
        let act_batch = journal_to_activity_batch(journal)?;
        let activity_count = act_batch.num_rows();
        self.insert_batch("experiments", &exp_batch)?;
        if activity_count > 0 {
            self.insert_batch("activity_results", &act_batch)?;
        }
        if let Some(ref load_result) = journal.load_result {
            let load_batch = journal_to_load_batch(&journal.experiment_id, load_result)?;
            self.insert_batch("load_results", &load_batch)?;
        }
        telemetry::event_journal_ingested(&journal.experiment_id, activity_count);
        Ok(true)
    }

    /// Ingest multiple journals, skipping duplicates.
    /// Returns the count of newly ingested journals.
    ///
    /// # Errors
    ///
    /// Returns an error if any individual journal ingestion fails.
    #[must_use = "callers must check the count of newly ingested journals"]
    pub fn ingest_journals(&self, journals: &[Journal]) -> Result<usize, AnalyticsError> {
        let mut count = 0;
        for journal in journals {
            if self.ingest_journal(journal)? {
                count += 1;
            }
        }
        // Record store gauges after batch ingestion
        if let Ok(stats) = self.stats() {
            telemetry::record_store_gauges(stats.experiment_count, stats.activity_count, None);
        }
        Ok(count)
    }

    /// # Errors
    ///
    /// Returns an error if the SQL query fails to execute.
    #[must_use = "callers must use the returned query rows"]
    pub fn query(&self, sql: &str) -> Result<Vec<QueryRow>, AnalyticsError> {
        let _span = telemetry::begin_query(sql);

        let mut stmt = self.conn.prepare(sql)?;
        let mut rows_iter = stmt.query(params![])?;
        let column_count = rows_iter
            .as_ref()
            .map_or(0, duckdb::Statement::column_count);
        let mut result = Vec::new();
        while let Some(row) = rows_iter.next()? {
            let mut values = Vec::with_capacity(column_count);
            for i in 0..column_count {
                let val: String = row
                    .get::<_, duckdb::types::Value>(i)
                    .map_or_else(|_| "NULL".to_string(), |v| format_value(&v));
                values.push(val);
            }
            result.push(QueryRow::from(values));
        }
        telemetry::event_query_executed(result.len(), column_count);
        Ok(result)
    }

    /// Execute a SQL query with a single bound string parameter (e.g. a `LIKE`
    /// pattern). The SQL must contain exactly one `?` placeholder.
    ///
    /// Use this instead of [`Self::query`] when the query includes a value
    /// derived from user input — binding via a parameter prevents SQL injection.
    ///
    /// # Errors
    ///
    /// Returns an error if the SQL query fails to execute.
    #[must_use = "callers must use the returned query rows"]
    pub fn query_with_param(
        &self,
        sql: &str,
        param: &str,
    ) -> Result<Vec<QueryRow>, AnalyticsError> {
        let _span = telemetry::begin_query(sql);

        let mut stmt = self.conn.prepare(sql)?;
        let mut rows_iter = stmt.query(params![param])?;
        let column_count = rows_iter
            .as_ref()
            .map_or(0, duckdb::Statement::column_count);
        let mut result = Vec::new();
        while let Some(row) = rows_iter.next()? {
            let mut values = Vec::with_capacity(column_count);
            for i in 0..column_count {
                let val: String = row
                    .get::<_, duckdb::types::Value>(i)
                    .map_or_else(|_| "NULL".to_string(), |v| format_value(&v));
                values.push(val);
            }
            result.push(QueryRow::from(values));
        }
        telemetry::event_query_executed(result.len(), column_count);
        Ok(result)
    }

    /// # Errors
    ///
    /// Returns an error if the SQL query fails to execute.
    #[must_use = "callers must use the returned column names"]
    pub fn query_columns(&self, sql: &str) -> Result<Vec<String>, AnalyticsError> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query(params![])?;
        let names = rows
            .as_ref()
            .map(duckdb::Statement::column_names)
            .unwrap_or_default();
        Ok(names)
    }

    /// # Errors
    ///
    /// Returns an error if the count query fails.
    #[must_use = "callers must use the returned experiment count"]
    pub fn experiment_count(&self) -> Result<usize, AnalyticsError> {
        let mut stmt = self.conn.prepare("SELECT count(*) FROM experiments")?;
        let count: i64 = stmt.query_row(params![], |row| row.get(0))?;
        // DuckDB count(*) is never negative; i64 → usize is safe on 64-bit targets.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(count as usize)
    }

    /// # Errors
    ///
    /// Returns an error if either count query fails.
    #[must_use = "callers must use the returned store statistics"]
    pub fn stats(&self) -> Result<StoreStats, AnalyticsError> {
        let exp_count = self.experiment_count()?;
        let mut stmt = self.conn.prepare("SELECT count(*) FROM activity_results")?;
        let act_count: i64 = stmt.query_row(params![], |row| row.get(0))?;
        Ok(StoreStats {
            experiment_count: exp_count,
            // DuckDB count(*) is never negative; i64 → usize is safe on 64-bit targets.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            activity_count: act_count as usize,
        })
    }

    /// Purge experiments (and their activities) older than `days` from now.
    /// Returns the number of experiments removed.
    ///
    /// # Errors
    ///
    /// Returns an error if any `DuckDB` operation fails.
    ///
    /// # Panics
    ///
    /// Panics if `days * 86_400_000_000_000` overflows an `i64`.
    #[must_use = "callers must check the count of purged experiments"]
    pub fn purge_older_than_days(&self, days: u32) -> Result<usize, AnalyticsError> {
        let _span = telemetry::begin_purge(days);

        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX);
        let retention_ns = i64::from(days)
            .checked_mul(86_400_000_000_000)
            .expect("retention period overflow");
        let cutoff_ns = now_ns.saturating_sub(retention_ns);

        // Delete activity results for old experiments first
        self.conn.execute(
            "DELETE FROM activity_results WHERE experiment_id IN \
             (SELECT experiment_id FROM experiments WHERE started_at_ns < ?)",
            params![cutoff_ns],
        )?;

        // Delete old experiments
        let mut stmt = self
            .conn
            .prepare("DELETE FROM experiments WHERE started_at_ns < ? RETURNING experiment_id")?;
        let mut rows = stmt.query(params![cutoff_ns])?;
        let mut count = 0;
        while rows.next()?.is_some() {
            count += 1;
        }
        let remaining = self.experiment_count().unwrap_or(0);
        telemetry::event_purge_completed(count, remaining);
        Ok(count)
    }

    /// Export both tables to Parquet files for backup.
    ///
    /// # Errors
    ///
    /// Returns an error if any `DuckDB` query or Parquet write fails.
    #[must_use = "callers must handle export errors"]
    pub fn export_tables(
        &self,
        experiments_path: &Path,
        activities_path: &Path,
    ) -> Result<(), AnalyticsError> {
        let _span = telemetry::begin_export(
            "parquet",
            &experiments_path
                .parent()
                .unwrap_or(experiments_path)
                .display()
                .to_string(),
        );

        let exp_batch = self.query_to_batch(
            "SELECT experiment_id, title, status, started_at_ns, ended_at_ns, \
             duration_ms, method_step_count, rollback_count, hypothesis_before_met, \
             hypothesis_after_met, estimate_accuracy, resilience_score FROM experiments",
        )?;
        let act_batch = self.query_to_batch(
            "SELECT experiment_id, name, activity_type, status, started_at_ns, \
             duration_ms, output, error, phase FROM activity_results",
        )?;
        export_parquet(&exp_batch, experiments_path)?;
        export_parquet(&act_batch, activities_path)?;

        let total_rows = exp_batch.num_rows() + act_batch.num_rows();
        let total_bytes = std::fs::metadata(experiments_path)
            .map(|m| m.len())
            .unwrap_or(0)
            + std::fs::metadata(activities_path)
                .map(|m| m.len())
                .unwrap_or(0);
        telemetry::event_export_completed("parquet", total_rows, total_bytes);

        Ok(())
    }

    /// Import from Parquet backup files. Wrapped in a transaction for atomicity.
    ///
    /// # Errors
    ///
    /// Returns an error if the Parquet read or `DuckDB` insert fails.
    #[must_use = "callers must handle import errors"]
    pub fn import_tables(
        &self,
        experiments_path: &Path,
        activities_path: &Path,
    ) -> Result<(), AnalyticsError> {
        let _span = telemetry::begin_import(
            &experiments_path
                .parent()
                .unwrap_or(experiments_path)
                .display()
                .to_string(),
        );

        self.conn.execute_batch("BEGIN TRANSACTION")?;
        match self.import_tables_inner(experiments_path, activities_path) {
            Ok(()) => {
                self.conn.execute_batch("COMMIT")?;
                let stats = self.stats().unwrap_or(StoreStats {
                    experiment_count: 0,
                    activity_count: 0,
                });
                telemetry::event_import_completed(stats.experiment_count, stats.activity_count);
                Ok(())
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    fn import_tables_inner(
        &self,
        experiments_path: &Path,
        activities_path: &Path,
    ) -> Result<(), AnalyticsError> {
        let exp_batches = import_parquet(experiments_path)?;
        for batch in &exp_batches {
            self.insert_batch("experiments", batch)?;
        }
        let act_batches = import_parquet(activities_path)?;
        for batch in &act_batches {
            self.insert_batch("activity_results", batch)?;
        }
        Ok(())
    }

    fn query_to_batch(&self, sql: &str) -> Result<RecordBatch, AnalyticsError> {
        let mut stmt = self.conn.prepare(sql)?;
        let arrow = stmt.query_arrow(params![])?;
        // Capture the schema before consuming the iterator so that an empty
        // result set uses the schema of the actual query, not the hardcoded
        // experiments schema (ANA-MED-8).
        let schema = arrow.get_schema();
        let batches: Vec<RecordBatch> = arrow.collect();
        if batches.is_empty() {
            Ok(RecordBatch::new_empty(schema))
        } else if batches.len() == 1 {
            batches.into_iter().next().ok_or_else(|| {
                AnalyticsError::Internal("query returned one batch but iterator was empty".into())
            })
        } else {
            let schema = batches[0].schema();
            Ok(arrow::compute::concat_batches(&schema, &batches)?)
        }
    }

    fn insert_batch(&self, table: &str, batch: &RecordBatch) -> Result<(), AnalyticsError> {
        let mut appender = self.conn.appender(table)?;
        appender.append_record_batch(batch.clone())?;
        appender.flush()?;
        Ok(())
    }
}

fn format_value(v: &duckdb::types::Value) -> String {
    match v {
        duckdb::types::Value::Null => "NULL".to_string(),
        duckdb::types::Value::Boolean(b) => b.to_string(),
        duckdb::types::Value::TinyInt(n) => n.to_string(),
        duckdb::types::Value::SmallInt(n) => n.to_string(),
        duckdb::types::Value::Int(n) => n.to_string(),
        duckdb::types::Value::BigInt(n) => n.to_string(),
        duckdb::types::Value::UBigInt(n) => n.to_string(),
        duckdb::types::Value::Float(f) => format!("{f:.2}"),
        duckdb::types::Value::Double(f) => format!("{f:.4}"),
        duckdb::types::Value::Text(s) => s.clone(),
        _ => format!("{v:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tumult_core::types::*;

    fn sample_journal(id: &str, status: ExperimentStatus) -> Journal {
        Journal {
            experiment_title: format!("Test {id}"),
            experiment_id: id.into(),
            status,
            started_at_ns: 1_774_980_000_000_000_000,
            ended_at_ns: 1_774_980_300_000_000_000,
            duration_ms: 300_000,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![ActivityResult {
                name: "action-1".into(),
                activity_type: ActivityType::Action,
                status: ActivityStatus::Succeeded,
                started_at_ns: 1_774_980_135_000_000_000,
                duration_ms: 500,
                output: Some("done".into()),
                error: None,
                trace_id: "t1".into(),
                span_id: "s1".into(),
            }],
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: Some(AnalysisResult {
                estimate_accuracy: Some(1.0),
                estimate_recovery_delta_s: None,
                trend: None,
                resilience_score: Some(0.95),
            }),
            regulatory: None,
        }
    }

    #[test]
    fn create_store() {
        let s = AnalyticsStore::in_memory().unwrap();
        assert_eq!(s.experiment_count().unwrap(), 0);
    }
    #[test]
    fn ingest_single() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        assert_eq!(s.experiment_count().unwrap(), 1);
    }
    #[test]
    fn ingest_multiple() {
        let s = AnalyticsStore::in_memory().unwrap();
        assert_eq!(
            s.ingest_journals(&[
                sample_journal("e1", ExperimentStatus::Completed),
                sample_journal("e2", ExperimentStatus::Deviated),
                sample_journal("e3", ExperimentStatus::Completed)
            ])
            .unwrap(),
            3
        );
        assert_eq!(s.experiment_count().unwrap(), 3);
    }
    #[test]
    fn query_by_status() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        s.ingest_journal(&sample_journal("e2", ExperimentStatus::Deviated))
            .unwrap();
        s.ingest_journal(&sample_journal("e3", ExperimentStatus::Completed))
            .unwrap();
        let rows = s
            .query("SELECT experiment_id FROM experiments WHERE status = 'completed'")
            .unwrap();
        assert_eq!(rows.len(), 2);
    }
    #[test]
    fn query_avg() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        let rows = s.query("SELECT avg(duration_ms) FROM experiments").unwrap();
        assert_eq!(rows.len(), 1);
    }
    #[test]
    fn query_activities() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        let rows = s
            .query("SELECT name, phase FROM activity_results WHERE phase = 'method'")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "action-1");
    }
    #[test]
    fn query_columns() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        let cols = s
            .query_columns("SELECT experiment_id, status FROM experiments")
            .unwrap();
        assert_eq!(cols, vec!["experiment_id", "status"]);
    }

    /// Regression test for ANA-MED-8: an empty query on `activity_results` must
    /// return a batch with the activity schema, not the experiments schema.
    #[test]
    fn empty_query_returns_correct_schema() {
        let s = AnalyticsStore::in_memory().unwrap();
        // No data ingested — both tables are empty.
        let batch = s
            .query_to_batch(
                "SELECT experiment_id, name, activity_type, status, started_at_ns, \
                 duration_ms, output, error, phase FROM activity_results",
            )
            .unwrap();
        assert_eq!(batch.num_rows(), 0);
        // The schema must contain 'activity_type' (an activity column),
        // not columns exclusive to the experiments table.
        let schema = batch.schema();
        let col_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert!(
            col_names.contains(&"activity_type"),
            "expected activity schema but got: {col_names:?}"
        );
        assert!(
            !col_names.contains(&"resilience_score"),
            "got experiments schema instead of activity schema: {col_names:?}"
        );
    }

    // ── Phase 4: Persistent store ─────────────────────────────

    #[test]
    fn open_persistent_creates_file() {
        let d = tempfile::TempDir::new().unwrap();
        let db_path = d.path().join("analytics.duckdb");
        let s = AnalyticsStore::open(&db_path).unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        assert_eq!(s.experiment_count().unwrap(), 1);
        drop(s);
        assert!(db_path.exists());
    }

    #[test]
    fn persistent_store_survives_reopen() {
        let d = tempfile::TempDir::new().unwrap();
        let db_path = d.path().join("analytics.duckdb");

        // Write
        {
            let s = AnalyticsStore::open(&db_path).unwrap();
            s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
                .unwrap();
            assert_eq!(s.experiment_count().unwrap(), 1);
        }

        // Reopen and verify data persisted
        {
            let s = AnalyticsStore::open(&db_path).unwrap();
            assert_eq!(s.experiment_count().unwrap(), 1);
            let rows = s.query("SELECT experiment_id FROM experiments").unwrap();
            assert_eq!(rows[0][0], "e1");
        }
    }

    #[test]
    fn default_path_returns_valid_path() {
        let path = AnalyticsStore::default_path();
        assert!(path.ends_with("analytics.duckdb"));
        assert!(path.to_str().unwrap().contains(".tumult"));
    }

    #[test]
    fn open_default_creates_directory() {
        // This test uses a temp directory to avoid polluting the real home
        let d = tempfile::TempDir::new().unwrap();
        let db_path = d.path().join("subdir").join("analytics.duckdb");
        let s = AnalyticsStore::open(&db_path).unwrap();
        assert_eq!(s.experiment_count().unwrap(), 0);
        assert!(db_path.exists());
    }

    // ── Phase 4: Incremental ingestion (dedup) ────────────────

    #[test]
    fn ingest_skips_duplicate_experiment_id() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        // Should only have 1 row, not 2
        assert_eq!(s.experiment_count().unwrap(), 1);
    }

    #[test]
    fn ingest_journals_returns_only_new_count() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        let ingested = s
            .ingest_journals(&[
                sample_journal("e1", ExperimentStatus::Completed), // duplicate
                sample_journal("e2", ExperimentStatus::Deviated),  // new
                sample_journal("e3", ExperimentStatus::Completed), // new
            ])
            .unwrap();
        assert_eq!(ingested, 2); // only 2 new
        assert_eq!(s.experiment_count().unwrap(), 3);
    }

    // ── Phase 4: WAL mode ─────────────────────────────────────

    #[test]
    fn persistent_store_is_functional_after_write_and_reopen() {
        let d = tempfile::TempDir::new().unwrap();
        let db_path = d.path().join("analytics.duckdb");

        // Write data and close
        {
            let s = AnalyticsStore::open(&db_path).unwrap();
            s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
                .unwrap();
            s.ingest_journal(&sample_journal("e2", ExperimentStatus::Deviated))
                .unwrap();
        }

        // Reopen — DuckDB uses WAL by default for file-backed databases
        {
            let s = AnalyticsStore::open(&db_path).unwrap();
            assert_eq!(s.experiment_count().unwrap(), 2);
            let rows = s.query("SELECT count(*) FROM activity_results").unwrap();
            assert_eq!(rows[0][0], "2");
        }
    }

    // ── Phase 4: Schema version tracking ──────────────────────

    #[test]
    fn schema_version_is_tracked() {
        let s = AnalyticsStore::in_memory().unwrap();
        let version = s.schema_version().unwrap();
        assert!(version >= 1, "schema version should be at least 1");
    }

    #[test]
    fn schema_version_persists_across_reopen() {
        let d = tempfile::TempDir::new().unwrap();
        let db_path = d.path().join("analytics.duckdb");

        {
            let s = AnalyticsStore::open(&db_path).unwrap();
            assert_eq!(s.schema_version().unwrap(), 1);
        }

        {
            let s = AnalyticsStore::open(&db_path).unwrap();
            assert_eq!(s.schema_version().unwrap(), 1);
        }
    }

    // ── Phase 4: Store statistics ─────────────────────────────

    #[test]
    fn store_stats_empty() {
        let s = AnalyticsStore::in_memory().unwrap();
        let stats = s.stats().unwrap();
        assert_eq!(stats.experiment_count, 0);
        assert_eq!(stats.activity_count, 0);
    }

    #[test]
    fn store_stats_after_ingestion() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        s.ingest_journal(&sample_journal("e2", ExperimentStatus::Deviated))
            .unwrap();
        let stats = s.stats().unwrap();
        assert_eq!(stats.experiment_count, 2);
        assert_eq!(stats.activity_count, 2); // 1 activity per journal
    }

    // ── Phase 4: Retention policy ─────────────────────────────

    #[test]
    fn purge_older_than_removes_old_experiments() {
        let s = AnalyticsStore::in_memory().unwrap();

        // Create journal with old timestamp (2020)
        let mut old = sample_journal("old-1", ExperimentStatus::Completed);
        old.started_at_ns = 1_577_836_800_000_000_000; // 2020-01-01

        // Create journal with recent timestamp
        let recent = sample_journal("new-1", ExperimentStatus::Completed);

        s.ingest_journal(&old).unwrap();
        s.ingest_journal(&recent).unwrap();
        assert_eq!(s.experiment_count().unwrap(), 2);

        // Purge experiments older than 30 days from now
        let purged = s.purge_older_than_days(30).unwrap();
        assert_eq!(purged, 1);
        assert_eq!(s.experiment_count().unwrap(), 1);

        // The remaining experiment should be the recent one
        let rows = s.query("SELECT experiment_id FROM experiments").unwrap();
        assert_eq!(rows[0][0], "new-1");
    }

    #[test]
    fn purge_also_removes_activity_results() {
        let s = AnalyticsStore::in_memory().unwrap();

        let mut old = sample_journal("old-1", ExperimentStatus::Completed);
        old.started_at_ns = 1_577_836_800_000_000_000; // 2020-01-01

        s.ingest_journal(&old).unwrap();
        s.ingest_journal(&sample_journal("new-1", ExperimentStatus::Completed))
            .unwrap();

        s.purge_older_than_days(30).unwrap();

        // Activity results for old experiment should also be gone
        let rows = s
            .query("SELECT count(*) FROM activity_results WHERE experiment_id = 'old-1'")
            .unwrap();
        assert_eq!(rows[0][0], "0");
    }

    // ── Phase 4: Export entire store ──────────────────────────

    #[test]
    fn export_store_to_parquet() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
            .unwrap();
        s.ingest_journal(&sample_journal("e2", ExperimentStatus::Deviated))
            .unwrap();

        let d = tempfile::TempDir::new().unwrap();
        let exp_path = d.path().join("experiments.parquet");
        let act_path = d.path().join("activities.parquet");

        s.export_tables(&exp_path, &act_path).unwrap();

        assert!(exp_path.exists());
        assert!(act_path.exists());
        assert!(std::fs::metadata(&exp_path).unwrap().len() > 0);
        assert!(std::fs::metadata(&act_path).unwrap().len() > 0);
    }

    // ── Phase 4: Import from Parquet ──────────────────────────

    #[test]
    fn import_from_parquet_roundtrip() {
        let d = tempfile::TempDir::new().unwrap();
        let exp_path = d.path().join("experiments.parquet");
        let act_path = d.path().join("activities.parquet");

        // Export from one store
        {
            let s = AnalyticsStore::in_memory().unwrap();
            s.ingest_journal(&sample_journal("e1", ExperimentStatus::Completed))
                .unwrap();
            s.ingest_journal(&sample_journal("e2", ExperimentStatus::Deviated))
                .unwrap();
            s.export_tables(&exp_path, &act_path).unwrap();
        }

        // Import into a fresh store
        {
            let s = AnalyticsStore::in_memory().unwrap();
            s.import_tables(&exp_path, &act_path).unwrap();
            assert_eq!(s.experiment_count().unwrap(), 2);

            let rows = s
                .query("SELECT experiment_id FROM experiments ORDER BY experiment_id")
                .unwrap();
            assert_eq!(rows[0][0], "e1");
            assert_eq!(rows[1][0], "e2");
        }
    }

    // ── Phase 4: Unique index enforcement ─────────────────────

    #[test]
    fn experiment_id_has_unique_index() {
        let s = AnalyticsStore::in_memory().unwrap();
        let result = s
            .query("SELECT count(*) FROM duckdb_indexes() WHERE table_name = 'experiments'")
            .unwrap();
        let idx_count: usize = result[0][0].parse().unwrap_or(0);
        assert!(idx_count >= 1, "experiments table should have an index");
    }

    #[test]
    fn load_result_ingested_into_duckdb() {
        use tumult_core::types::{LoadResult, LoadTool};

        let s = AnalyticsStore::in_memory().unwrap();
        let mut journal = sample_journal("load-test-1", ExperimentStatus::Completed);
        journal.load_result = Some(LoadResult {
            tool: LoadTool::K6,
            started_at_ns: 1_000_000_000,
            ended_at_ns: 11_000_000_000,
            duration_s: 10.0,
            vus: 5,
            throughput_rps: 100.0,
            latency_p50_ms: 15.0,
            latency_p95_ms: 150.0,
            latency_p99_ms: 500.0,
            error_rate: 0.02,
            total_requests: 1000,
            thresholds_met: true,
        });
        s.ingest_journal(&journal).unwrap();

        let rows = s
            .query("SELECT experiment_id, tool, vus, throughput_rps, latency_p95_ms, error_rate, total_requests FROM load_results")
            .unwrap();
        assert_eq!(rows.len(), 1, "should have 1 load result row");
        assert_eq!(rows[0][0], "load-test-1");
        assert_eq!(rows[0][1], "k6");
        assert_eq!(rows[0][2], "5");
    }

    #[test]
    fn no_load_result_row_when_none() {
        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("no-load", ExperimentStatus::Completed))
            .unwrap();
        let rows = s.query("SELECT count(*) FROM load_results").unwrap();
        assert_eq!(rows[0][0], "0");
    }

    #[test]
    fn query_with_param_binds_like_pattern() {
        use tumult_core::types::ExperimentStatus;

        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("alpha-1", ExperimentStatus::Completed))
            .unwrap();
        s.ingest_journal(&sample_journal("beta-2", ExperimentStatus::Completed))
            .unwrap();

        // Pattern matches only the first journal's title (which equals its ID).
        let rows = s
            .query_with_param(
                "SELECT experiment_id FROM experiments WHERE lower(title) LIKE ?",
                "%alpha%",
            )
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "alpha-1");
    }

    #[test]
    fn query_with_param_single_quote_in_pattern_does_not_cause_error() {
        use tumult_core::types::ExperimentStatus;

        let s = AnalyticsStore::in_memory().unwrap();
        s.ingest_journal(&sample_journal("no-match", ExperimentStatus::Completed))
            .unwrap();

        // A single quote in the bind value must not trigger a SQL error.
        let rows = s
            .query_with_param(
                "SELECT experiment_id FROM experiments WHERE lower(title) LIKE ?",
                "%o'clock%",
            )
            .unwrap();
        assert!(rows.is_empty());
    }
}
