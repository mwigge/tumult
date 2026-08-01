//! The write side of the store (split out of the crate root).

use duckdb::{params, Connection};

use crate::error::StoreError;
use crate::rows::{ImportBatch, LogRow, MetricGaugeRow, MetricHistogramRow, MetricSumRow, SpanRow};
use crate::store::{attrs_json, migrate, query_json_rows, with_tx};

/// The write side of the store. Hold at most one per process (the ingest
/// daemon funnels every write through a channel onto a single `Writer`).
pub struct Writer {
    pub(crate) conn: Connection,
}

impl Writer {
    pub(crate) fn migrate(&self) -> Result<(), StoreError> {
        migrate(&self.conn).map_err(StoreError::from)
    }

    /// Recorded schema version.
    ///
    /// # Errors
    /// Returns an error if the metadata table cannot be read.
    pub fn schema_version(&self) -> Result<i64, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM schema_meta WHERE key = 'version'")?;
        stmt.query_row(params![], |row| row.get(0))
            .map_err(StoreError::from)
    }

    /// Insert a batch of span rows in one transaction.
    ///
    /// # Errors
    /// Returns an error if the batch fails to insert (the transaction is rolled back).
    pub fn insert_spans(&self, rows: &[SpanRow]) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            let mut stmt = self.conn.prepare(
                "INSERT INTO spans VALUES (
                    ?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    CAST(? AS JSON))",
            )?;
            for r in rows {
                // An empty events string is not valid JSON; normalize to `[]`.
                let events = if r.events.is_empty() {
                    "[]"
                } else {
                    r.events.as_str()
                };
                stmt.execute(params![
                    r.ts_ns,
                    r.trace_id,
                    r.span_id,
                    r.parent_span_id,
                    r.span_name,
                    r.span_kind,
                    r.duration_ns,
                    r.status_code,
                    r.status_message,
                    r.service_name,
                    r.service_version,
                    r.experiment_id,
                    r.experiment_name,
                    r.outcome_status,
                    r.fault_type,
                    r.fault_subtype,
                    r.fault_severity,
                    r.blast_radius,
                    r.target_system,
                    r.target_technology,
                    r.target_environment,
                    r.plugin_name,
                    r.hypothesis_met,
                    r.recovery_time_s,
                    attrs_json(&r.span_attrs)?,
                    attrs_json(&r.resource_attrs)?,
                    events,
                ])?;
            }
            Ok(())
        })
    }

    /// Insert a batch of log rows in one transaction.
    ///
    /// # Errors
    /// Returns an error if the batch fails to insert (the transaction is rolled back).
    pub fn insert_logs(&self, rows: &[LogRow]) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            let mut stmt = self.conn.prepare(
                "INSERT INTO logs VALUES (?,?,?,?,?,?,
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)))",
            )?;
            for r in rows {
                stmt.execute(params![
                    r.ts_ns,
                    r.severity_text,
                    r.body,
                    r.trace_id,
                    r.span_id,
                    r.service_name,
                    attrs_json(&r.log_attrs)?,
                    attrs_json(&r.resource_attrs)?,
                ])?;
            }
            Ok(())
        })
    }

    /// Insert a batch of sum data points in one transaction.
    ///
    /// # Errors
    /// Returns an error if the batch fails to insert (the transaction is rolled back).
    pub fn insert_metric_sums(&self, rows: &[MetricSumRow]) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            let mut stmt = self.conn.prepare(
                "INSERT INTO metric_sums VALUES (?,?,?,?,?,?,
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)))",
            )?;
            for r in rows {
                stmt.execute(params![
                    r.ts_ns,
                    r.metric_name,
                    r.value,
                    r.experiment_name,
                    r.outcome_status,
                    r.plugin_name,
                    attrs_json(&r.attrs)?,
                    attrs_json(&r.resource_attrs)?,
                ])?;
            }
            Ok(())
        })
    }

    /// Insert a batch of gauge data points in one transaction.
    ///
    /// # Errors
    /// Returns an error if the batch fails to insert (the transaction is rolled back).
    pub fn insert_metric_gauges(&self, rows: &[MetricGaugeRow]) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            let mut stmt = self.conn.prepare(
                "INSERT INTO metric_gauges VALUES (?,?,?,?,?,?,
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)))",
            )?;
            for r in rows {
                stmt.execute(params![
                    r.ts_ns,
                    r.metric_name,
                    r.value,
                    r.experiment_name,
                    r.outcome_status,
                    r.plugin_name,
                    attrs_json(&r.attrs)?,
                    attrs_json(&r.resource_attrs)?,
                ])?;
            }
            Ok(())
        })
    }

    /// Insert a batch of histogram data points in one transaction.
    ///
    /// # Errors
    /// Returns an error if the batch fails to insert (the transaction is rolled back).
    pub fn insert_metric_histograms(&self, rows: &[MetricHistogramRow]) -> Result<(), StoreError> {
        with_tx(&self.conn, || {
            let mut stmt = self.conn.prepare(
                "INSERT INTO metric_histograms VALUES (?,?,?,?,?,?,
                    CAST(? AS BIGINT[]), CAST(? AS DOUBLE[]),
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    CAST(json(?) AS MAP(VARCHAR,VARCHAR)),
                    ?,?,?)",
            )?;
            for r in rows {
                stmt.execute(params![
                    r.ts_ns,
                    r.metric_name,
                    r.count,
                    r.sum,
                    r.min,
                    r.max,
                    serde_json::to_string(&r.bucket_counts)?,
                    serde_json::to_string(&r.explicit_bounds)?,
                    attrs_json(&r.attrs)?,
                    attrs_json(&r.resource_attrs)?,
                    r.experiment_name,
                    r.outcome_status,
                    r.plugin_name,
                ])?;
            }
            Ok(())
        })
    }

    /// Record a manual import batch in `import_batches`.
    ///
    /// # Errors
    /// Returns an error if the row fails to insert.
    pub fn record_import_batch(&self, batch: &ImportBatch) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO import_batches VALUES (?,?,?,?,?)",
            params![
                batch.id,
                batch.source,
                batch.imported_at_ns,
                batch.rows,
                batch.label
            ],
        )?;
        Ok(())
    }

    /// Ingest an experiment journal into the analytics tables
    /// (`experiments` / `activity_results` / `load_results` plus the
    /// `ChaosGraph` nodes/edges), on this single-writer connection.
    ///
    /// Same semantics as
    /// [`AnalyticsStore::ingest_journal_with_experiment`](crate::duckdb_store::AnalyticsStore::ingest_journal_with_experiment):
    /// the whole journal commits atomically and an already-known
    /// `experiment_id` is skipped as a duplicate. Pass `Some(experiment)` to
    /// enrich the graph with the full fault/service model.
    ///
    /// # Errors
    /// Returns an error if the insert or Arrow conversion fails.
    pub fn ingest_journal(
        &self,
        journal: &tumult_core::types::Journal,
        experiment: Option<&tumult_core::types::Experiment>,
    ) -> Result<bool, StoreError> {
        crate::duckdb_store::ingest_journal_with_experiment(&self.conn, journal, experiment)
            .map_err(|e| StoreError::Internal(e.to_string()))
    }

    /// Raw parameterized statement returning affected-row count — internal
    /// escape hatch (lake retention deletes, lake tests). Not stable public
    /// API.
    #[doc(hidden)]
    pub fn execute(&self, sql: &str, p: impl duckdb::Params) -> Result<usize, StoreError> {
        Ok(self.conn.execute(sql, p)?)
    }

    /// JSON-rows query on the write connection — crate-internal, used by
    /// snapshot-retention fingerprint checks in the lake.
    pub(crate) fn query_json_rows(&self, sql: &str) -> Result<Vec<serde_json::Value>, StoreError> {
        query_json_rows(&self.conn, sql)
    }
}
