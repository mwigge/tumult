//! Retention, export, and import maintenance operations.

use std::path::Path;
use std::sync::Arc;

use arrow::array::ArrayRef;
use arrow::datatypes::Schema;
use arrow::record_batch::RecordBatch;

use duckdb::params;

use crate::arrow_convert::{
    activity_results_schema, experiments_schema, journal_probe_samples_schema, load_results_schema,
};
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

        let exp_batch = self.query_to_batch(&select_of(&experiments_schema(), "experiments"))?;
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

    /// Export the load results and the probe samples (every window of
    /// every load among them) to Parquet files: with the experiments,
    /// the evidence a performance verdict is computed from.
    ///
    /// # Errors
    ///
    /// Returns an error if any `DuckDB` query or Parquet write fails.
    #[must_use = "callers must handle export errors"]
    pub fn export_load_tables(
        &self,
        load_results_path: &Path,
        probe_samples_path: &Path,
    ) -> Result<(), AnalyticsError> {
        let loads = self.query_to_batch(&select_of(&load_results_schema(), "load_results"))?;
        let samples =
            self.query_to_batch(&select_of(&journal_probe_samples_schema(), "probe_samples"))?;
        export_parquet(&loads, load_results_path)?;
        export_parquet(&samples, probe_samples_path)?;
        telemetry::event_export_completed(
            "parquet",
            loads.num_rows() + samples.num_rows(),
            std::fs::metadata(load_results_path).map_or(0, |m| m.len())
                + std::fs::metadata(probe_samples_path).map_or(0, |m| m.len()),
        );
        Ok(())
    }

    /// Import what [`AnalyticsStore::export_load_tables`] wrote, in one
    /// transaction. A file written before a column existed imports with
    /// that column unknown. Rows the lake already holds (by
    /// [`LOAD_RESULTS`] and [`PROBE_SAMPLES`]) are left as they are, so
    /// importing a backup twice, or into a lake that has some of its
    /// runs, adds each row once.
    ///
    /// # Errors
    ///
    /// Returns an error if the Parquet read or `DuckDB` insert fails
    /// (nothing is kept).
    #[must_use = "callers must handle import errors"]
    pub fn import_load_tables(
        &self,
        load_results_path: &Path,
        probe_samples_path: &Path,
    ) -> Result<(), AnalyticsError> {
        self.in_transaction(|| {
            self.import_new(&LOAD_RESULTS, &load_results_schema(), load_results_path)?;
            self.import_new(
                &PROBE_SAMPLES,
                &journal_probe_samples_schema(),
                probe_samples_path,
            )
        })
    }

    /// Run `work` in one transaction: kept when it succeeds, undone when
    /// it fails.
    fn in_transaction(
        &self,
        work: impl FnOnce() -> Result<(), AnalyticsError>,
    ) -> Result<(), AnalyticsError> {
        self.conn.execute_batch("BEGIN TRANSACTION")?;
        if let Err(e) = work() {
            if let Err(rollback) = self.conn.execute_batch("ROLLBACK") {
                tracing::warn!(error.message = %rollback, "lake.import.rollback_failed");
            }
            return Err(e);
        }
        self.conn.execute_batch("COMMIT")?;
        Ok(())
    }

    /// Import the rows of `path` the table does not hold yet: the file is
    /// read into a staging table of the same layout, and the rows whose
    /// natural key the table lacks are inserted from it. Rows that share
    /// a key inside one file are all kept.
    fn import_new(
        &self,
        into: &Imported,
        schema: &Schema,
        path: &Path,
    ) -> Result<(), AnalyticsError> {
        let table = into.table;
        self.conn.execute_batch(&format!(
            "CREATE OR REPLACE TEMP TABLE {STAGE} AS SELECT * FROM {table} LIMIT 0"
        ))?;
        for batch in &import_parquet(path)? {
            self.insert_batch(STAGE, &pad_batch(batch, schema)?)?;
        }
        self.conn.execute_batch(&format!(
            "INSERT INTO {table} SELECT * FROM {STAGE} AS s WHERE NOT EXISTS (\
             SELECT 1 FROM {table} AS t WHERE {}); DROP TABLE {STAGE}",
            into.same_row()
        ))?;
        Ok(())
    }

    /// Import from Parquet backup files. Wrapped in a transaction for
    /// atomicity. Experiments and activities the lake already holds are
    /// left as they are, so a second import of the same backup adds
    /// nothing.
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

        self.in_transaction(|| {
            self.import_new(&EXPERIMENTS, &experiments_schema(), experiments_path)?;
            self.import_new(&ACTIVITIES, &activity_results_schema(), activities_path)
        })?;
        let stats = self.stats().unwrap_or(StoreStats {
            experiment_count: 0,
            activity_count: 0,
        });
        telemetry::event_import_completed(stats.experiment_count, stats.activity_count);
        Ok(())
    }
}

/// The staging table an import reads a file into.
const STAGE: &str = "import_stage";

/// A table an import adds rows to, and the columns that say two rows are
/// the same row: its natural key.
struct Imported {
    table: &'static str,
    key: &'static [&'static str],
}

const EXPERIMENTS: Imported = Imported {
    table: "experiments",
    key: &["experiment_id"],
};
const ACTIVITIES: Imported = Imported {
    table: "activity_results",
    key: &[
        "experiment_id",
        "name",
        "activity_type",
        "phase",
        "started_at_ns",
    ],
};
/// A run has one load result.
const LOAD_RESULTS: Imported = Imported {
    table: "load_results",
    key: &["experiment_id"],
};
/// One value of one series of one run at one instant.
const PROBE_SAMPLES: Imported = Imported {
    table: "probe_samples",
    key: &["experiment_id", "probe_name", "phase", "ts_ns"],
};

impl Imported {
    /// The predicate that row `s` of the staging table is row `t` of the
    /// table.
    fn same_row(&self) -> String {
        let same: Vec<String> = self
            .key
            .iter()
            .map(|column| format!("t.{column} IS NOT DISTINCT FROM s.{column}"))
            .collect();
        same.join(" AND ")
    }
}

/// `SELECT` of the columns `schema` names from `table`, in its order: an
/// export follows the layout its import appends by, column for column.
fn select_of(schema: &Schema, table: &str) -> String {
    let columns: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    format!("SELECT {} FROM {table}", columns.join(", "))
}

/// Widen a batch exported before the table gained its latest columns
/// (an `experiments` export before schema v16 has 12) to `schema` by
/// appending null columns, so old Parquet backups still import. Batches
/// already at the current width pass through.
fn pad_batch(batch: &RecordBatch, schema: &Schema) -> Result<RecordBatch, AnalyticsError> {
    if batch.num_columns() >= schema.fields().len() {
        return Ok(batch.clone());
    }
    let rows = batch.num_rows();
    let mut fields = batch.schema().fields().to_vec();
    let mut columns: Vec<ArrayRef> = batch.columns().to_vec();
    for field in schema.fields().iter().skip(batch.num_columns()) {
        fields.push(Arc::new(field.as_ref().clone().with_nullable(true)));
        columns.push(arrow::array::new_null_array(field.data_type(), rows));
    }
    Ok(RecordBatch::try_new(
        Arc::new(Schema::new(fields)),
        columns,
    )?)
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

    /// A run with everything a performance verdict reads: the version it
    /// recorded, how its load was executed, and its windows.
    fn load_run(id: &str) -> Journal {
        let window = |ts_ns: i64| LoadSample {
            ts_ns,
            duration_s: 5.0,
            vus: 4,
            throughput_rps: 40.0,
            latency_p50_ms: 20.0,
            latency_p95_ms: 70.0,
            latency_p99_ms: 2000.0,
            error_rate: 0.25,
            requests: 200,
            counts: Some(RequestCounts {
                sent: 200,
                ok: 150,
                timed_out: 50,
                ..RequestCounts::default()
            }),
            request_timeout_ms: Some(2000.0),
        };
        let mut journal = sample_journal(id, ExperimentStatus::Completed);
        journal.target_version = TargetVersion::observed(vec![
            VersionShare {
                version: "1.5.0".into(),
                requests: 70,
            },
            VersionShare {
                version: "1.4.0".into(),
                requests: 30,
            },
        ]);
        journal.load_result = Some(LoadResult {
            tool: LoadTool::Oha,
            started_at_ns: 1,
            ended_at_ns: 2,
            duration_s: 10.0,
            vus: 4,
            throughput_rps: 40.0,
            latency_p50_ms: 20.0,
            latency_p95_ms: 70.0,
            latency_p99_ms: 2000.0,
            error_rate: 0.25,
            total_requests: 400,
            thresholds_met: false,
            samples: vec![window(1), window(2)],
            execution: LoadExecution {
                planned_windows: Some(2),
                completed_windows: Some(2),
                requests: Some(RequestCounts {
                    sent: 400,
                    ok: 300,
                    timed_out: 100,
                    ..RequestCounts::default()
                }),
                censored_windows: 2,
                ..LoadExecution::default()
            },
        });
        journal
    }

    const EVIDENCE: [&str; 3] = [
        "SELECT * FROM experiments ORDER BY experiment_id",
        "SELECT * FROM load_results ORDER BY experiment_id",
        "SELECT * FROM probe_samples ORDER BY experiment_id, ts_ns, probe_name",
    ];

    fn evidence(s: &AnalyticsStore) -> Vec<Vec<String>> {
        EVIDENCE
            .iter()
            .map(|sql| {
                let rows = s.query(sql).unwrap();
                rows.iter().map(|row| row.join("|")).collect()
            })
            .collect()
    }

    #[test]
    fn an_export_and_import_keep_the_evidence_of_a_performance_verdict() {
        let d = tempfile::TempDir::new().unwrap();
        let path = |name: &str| d.path().join(name);
        let exported = AnalyticsStore::in_memory().unwrap();
        exported
            .ingest_journals(&[load_run("r1"), load_run("r2")])
            .unwrap();
        exported
            .export_tables(&path("experiments.parquet"), &path("activities.parquet"))
            .unwrap();
        exported
            .export_load_tables(
                &path("load_results.parquet"),
                &path("probe_samples.parquet"),
            )
            .unwrap();

        let imported = AnalyticsStore::in_memory().unwrap();
        imported
            .import_tables(&path("experiments.parquet"), &path("activities.parquet"))
            .unwrap();
        imported
            .import_load_tables(
                &path("load_results.parquet"),
                &path("probe_samples.parquet"),
            )
            .unwrap();

        let before = evidence(&exported);
        assert_eq!(before[0].len(), 2);
        assert!(before[0][0].contains("|1.5.0|mixed|[{"), "{:?}", before[0]);
        assert!(
            before[1][0].ends_with("|400|300|100|0|0|0|0|0|2"),
            "{:?}",
            before[1]
        );
        assert_eq!(before[2].len(), 2 * 2 * 10);
        assert_eq!(evidence(&imported), before);
    }

    /// Export everything `store` holds into `dir`.
    fn export_all(store: &AnalyticsStore, dir: &std::path::Path) {
        let path = |name: &str| dir.join(name);
        store
            .export_tables(&path("experiments.parquet"), &path("activities.parquet"))
            .unwrap();
        store
            .export_load_tables(
                &path("load_results.parquet"),
                &path("probe_samples.parquet"),
            )
            .unwrap();
    }

    fn import_all(store: &AnalyticsStore, dir: &std::path::Path) {
        let path = |name: &str| dir.join(name);
        store
            .import_tables(&path("experiments.parquet"), &path("activities.parquet"))
            .unwrap();
        store
            .import_load_tables(
                &path("load_results.parquet"),
                &path("probe_samples.parquet"),
            )
            .unwrap();
    }

    const COUNTS: &str = "SELECT (SELECT count(*) FROM experiments), \
         (SELECT count(*) FROM activity_results), (SELECT count(*) FROM load_results), \
         (SELECT count(*) FROM probe_samples)";

    fn counts(store: &AnalyticsStore) -> String {
        store.query(COUNTS).unwrap()[0].join("|")
    }

    #[test]
    fn importing_a_backup_twice_adds_nothing_the_second_time() {
        let d = tempfile::TempDir::new().unwrap();
        let exported = AnalyticsStore::in_memory().unwrap();
        exported
            .ingest_journals(&[load_run("r1"), load_run("r2")])
            .unwrap();
        export_all(&exported, d.path());

        let imported = AnalyticsStore::in_memory().unwrap();
        import_all(&imported, d.path());
        let once = evidence(&imported);
        import_all(&imported, d.path());
        assert_eq!(counts(&imported), counts(&exported));
        assert_eq!(evidence(&imported), once);
    }

    #[test]
    fn importing_into_a_lake_that_holds_some_of_the_runs_adds_the_others() {
        let d = tempfile::TempDir::new().unwrap();
        let exported = AnalyticsStore::in_memory().unwrap();
        exported
            .ingest_journals(&[load_run("r1"), load_run("r2")])
            .unwrap();
        export_all(&exported, d.path());

        // The lake already holds r1, and a run the backup knows nothing of.
        let lake = AnalyticsStore::in_memory().unwrap();
        lake.ingest_journals(&[load_run("r1"), load_run("r3")])
            .unwrap();
        import_all(&lake, d.path());

        let all = AnalyticsStore::in_memory().unwrap();
        all.ingest_journals(&[load_run("r1"), load_run("r2"), load_run("r3")])
            .unwrap();
        assert_eq!(counts(&lake), counts(&all));
        assert_eq!(evidence(&lake), evidence(&all));
    }

    #[test]
    fn load_results_exported_before_the_request_counts_still_import() {
        use super::{export_parquet, journal_probe_samples_schema};

        let d = tempfile::TempDir::new().unwrap();
        let (loads, samples) = (d.path().join("l.parquet"), d.path().join("s.parquet"));
        let older = AnalyticsStore::in_memory().unwrap();
        older.ingest_journal(&load_run("r1")).unwrap();
        let narrow = older
            .query_to_batch(
                "SELECT experiment_id, tool, started_at_ns, ended_at_ns, duration_s, vus, \
                 throughput_rps, latency_p50_ms, latency_p95_ms, latency_p99_ms, error_rate, \
                 total_requests, thresholds_met FROM load_results",
            )
            .unwrap();
        export_parquet(&narrow, &loads).unwrap();
        let none = arrow::record_batch::RecordBatch::new_empty(std::sync::Arc::new(
            journal_probe_samples_schema(),
        ));
        export_parquet(&none, &samples).unwrap();

        let s = AnalyticsStore::in_memory().unwrap();
        s.import_load_tables(&loads, &samples).unwrap();
        let rows = s
            .query(
                "SELECT experiment_id, total_requests, aborted IS NULL, requests_sent IS NULL \
                 FROM load_results",
            )
            .unwrap();
        assert_eq!(rows[0].join("|"), "r1|400|true|true");
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
