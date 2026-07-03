//! Retention, export, and import maintenance operations.

use std::path::Path;

use duckdb::params;

use crate::error::AnalyticsError;
use crate::export::{export_parquet, import_parquet};
use crate::telemetry;

use super::{AnalyticsStore, StoreStats};

impl AnalyticsStore {
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
        let total_bytes = std::fs::metadata(experiments_path).map_or(0, |m| m.len())
            + std::fs::metadata(activities_path).map_or(0, |m| m.len());
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
}

#[cfg(test)]
mod tests {
    use super::super::sample_journal;
    use super::super::AnalyticsStore;
    use tumult_core::types::*;

    #[test]
    fn purge_older_than_removes_old_experiments() {
        let s = AnalyticsStore::in_memory().unwrap();

        // Create journal with old timestamp (2020)
        let mut old = sample_journal("old-1", ExperimentStatus::Completed);
        old.started_at_ns = 1_577_836_800_000_000_000; // 2020-01-01

        // Create journal with recent timestamp
        let mut recent = sample_journal("new-1", ExperimentStatus::Completed);
        recent.started_at_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX);

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
        let mut recent = sample_journal("new-1", ExperimentStatus::Completed);
        recent.started_at_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(i64::MAX);
        s.ingest_journal(&recent).unwrap();

        s.purge_older_than_days(30).unwrap();

        // Activity results for old experiment should also be gone
        let rows = s
            .query("SELECT count(*) FROM activity_results WHERE experiment_id = 'old-1'")
            .unwrap();
        assert_eq!(rows[0][0], "0");
    }

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
}
