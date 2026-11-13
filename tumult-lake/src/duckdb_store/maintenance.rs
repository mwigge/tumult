//! Retention, export, and import maintenance operations.

use std::path::Path;
use std::sync::Arc;

use arrow::array::ArrayRef;
use arrow::record_batch::RecordBatch;

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
    /// Returns an error if any `DuckDB` operation fails, or if the retention
    /// period overflows `i64` nanoseconds (requires `days > 106_751`).
    #[must_use = "callers must check the count of purged experiments"]
    pub fn purge_older_than_days(&self, days: u32) -> Result<usize, AnalyticsError> {
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
             hypothesis_after_met, estimate_accuracy, resilience_score, \
             test_kind, rto_target_s, rpo_target_s, rto_measured_s, rpo_measured_s \
             FROM experiments",
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
            self.insert_batch("experiments", &pad_experiment_batch(batch)?)?;
        }
        let act_batches = import_parquet(activities_path)?;
        for batch in &act_batches {
            self.insert_batch("activity_results", batch)?;
        }
        Ok(())
    }
}

/// Widen an `experiments` batch exported before schema v16 (12 columns) to
/// the current layout by appending null columns, so old Parquet backups
/// still import. Batches already at the current width pass through.
fn pad_experiment_batch(batch: &RecordBatch) -> Result<RecordBatch, AnalyticsError> {
    let schema = crate::arrow_convert::experiments_schema();
    if batch.num_columns() >= schema.fields().len() {
        return Ok(batch.clone());
    }
    let rows = batch.num_rows();
    let mut columns: Vec<ArrayRef> = batch.columns().to_vec();
    for field in schema.fields().iter().skip(batch.num_columns()) {
        columns.push(arrow::array::new_null_array(field.data_type(), rows));
    }
    Ok(RecordBatch::try_new(Arc::new(schema), columns)?)
}

#[cfg(test)]
mod tests {
    use super::super::sample_journal;
    use super::super::AnalyticsStore;
    use crate::error::AnalyticsError;
    use tumult_core::types::*;

    #[test]
    fn purge_retention_overflow_is_checked_error_not_panic() {
        let s = AnalyticsStore::in_memory().unwrap();
        let err = s.purge_older_than_days(u32::MAX).unwrap_err();
        assert!(
            matches!(err, AnalyticsError::Internal(ref msg) if msg.contains("overflows")),
            "expected Internal overflow error, got {err:?}"
        );
    }

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
    fn import_accepts_pre_v16_experiments_parquet() {
        use super::export_parquet;
        use arrow::array::{
            ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray, UInt64Array,
        };
        use arrow::datatypes::{DataType, Field, Schema};
        use arrow::record_batch::RecordBatch;
        use std::sync::Arc;

        let d = tempfile::TempDir::new().unwrap();
        let exp_path = d.path().join("experiments.parquet");
        let act_path = d.path().join("activities.parquet");

        // A 12-column batch, the shape every export before v16 produced.
        let legacy = Schema::new(vec![
            Field::new("experiment_id", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
            Field::new("status", DataType::Utf8, false),
            Field::new("started_at_ns", DataType::Int64, false),
            Field::new("ended_at_ns", DataType::Int64, false),
            Field::new("duration_ms", DataType::UInt64, false),
            Field::new("method_step_count", DataType::Int64, false),
            Field::new("rollback_count", DataType::Int64, false),
            Field::new("hypothesis_before_met", DataType::Boolean, true),
            Field::new("hypothesis_after_met", DataType::Boolean, true),
            Field::new("estimate_accuracy", DataType::Float64, true),
            Field::new("resilience_score", DataType::Float64, true),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(legacy),
            vec![
                Arc::new(StringArray::from(vec!["old-1"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["old"])),
                Arc::new(StringArray::from(vec!["completed"])),
                Arc::new(Int64Array::from(vec![1])),
                Arc::new(Int64Array::from(vec![2])),
                Arc::new(UInt64Array::from(vec![1u64])),
                Arc::new(Int64Array::from(vec![1])),
                Arc::new(Int64Array::from(vec![0])),
                Arc::new(BooleanArray::from(vec![Some(true)])),
                Arc::new(BooleanArray::from(vec![Some(true)])),
                Arc::new(Float64Array::from(vec![None::<f64>])),
                Arc::new(Float64Array::from(vec![None::<f64>])),
            ],
        )
        .unwrap();
        export_parquet(&batch, &exp_path).unwrap();
        let empty =
            RecordBatch::new_empty(Arc::new(crate::arrow_convert::activity_results_schema()));
        export_parquet(&empty, &act_path).unwrap();

        let s = AnalyticsStore::in_memory().unwrap();
        s.import_tables(&exp_path, &act_path).unwrap();
        let rows = s
            .query("SELECT experiment_id, test_kind IS NULL FROM experiments")
            .unwrap();
        assert_eq!(rows[0][0], "old-1");
        assert_eq!(rows[0][1], "true");
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
