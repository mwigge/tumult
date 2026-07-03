//! Read-side operations: SQL queries, counts, and statistics.

use arrow::record_batch::RecordBatch;
use duckdb::params;

use crate::error::AnalyticsError;
use crate::query_row::QueryRow;
use crate::telemetry;

use super::{AnalyticsStore, StoreStats};

impl AnalyticsStore {
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

    pub(crate) fn query_to_batch(&self, sql: &str) -> Result<RecordBatch, AnalyticsError> {
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
    use super::super::sample_journal;
    use super::super::AnalyticsStore;
    use tumult_core::types::*;

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
