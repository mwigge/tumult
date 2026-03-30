//! DuckDB embedded analytics store.
//!
//! Loads Arrow RecordBatches into DuckDB for SQL analytics.
//! DuckDB reads Arrow data zero-copy via its Arrow scanner.

use std::path::Path;

use arrow::record_batch::RecordBatch;
use duckdb::{params, Connection};

use tumult_core::types::Journal;

use crate::arrow_convert::{journal_to_activity_batch, journal_to_experiment_batch};
use crate::error::AnalyticsError;

/// Embedded analytics store backed by DuckDB.
pub struct AnalyticsStore {
    conn: Connection,
}

impl AnalyticsStore {
    /// Create an in-memory analytics store.
    pub fn in_memory() -> Result<Self, AnalyticsError> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.create_tables()?;
        Ok(store)
    }

    /// Create a persistent analytics store at the given path.
    pub fn open(path: &Path) -> Result<Self, AnalyticsError> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.create_tables()?;
        Ok(store)
    }

    fn create_tables(&self) -> Result<(), AnalyticsError> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS experiments (
                experiment_id VARCHAR NOT NULL,
                title VARCHAR NOT NULL,
                status VARCHAR NOT NULL,
                started_at_ns BIGINT NOT NULL,
                ended_at_ns BIGINT NOT NULL,
                duration_ms UBIGINT NOT NULL,
                method_step_count BIGINT NOT NULL,
                rollback_count BIGINT NOT NULL,
                hypothesis_before_met BOOLEAN,
                hypothesis_after_met BOOLEAN,
                estimate_accuracy DOUBLE,
                resilience_score DOUBLE
            );
            CREATE TABLE IF NOT EXISTS activity_results (
                experiment_id VARCHAR NOT NULL,
                name VARCHAR NOT NULL,
                activity_type VARCHAR NOT NULL,
                status VARCHAR NOT NULL,
                started_at_ns BIGINT NOT NULL,
                duration_ms UBIGINT NOT NULL,
                output VARCHAR,
                error VARCHAR,
                phase VARCHAR NOT NULL
            );",
        )?;
        Ok(())
    }

    /// Ingest a journal into the analytics store.
    pub fn ingest_journal(&self, journal: &Journal) -> Result<(), AnalyticsError> {
        let exp_batch = journal_to_experiment_batch(journal)?;
        let act_batch = journal_to_activity_batch(journal)?;

        self.insert_batch("experiments", &exp_batch)?;
        if act_batch.num_rows() > 0 {
            self.insert_batch("activity_results", &act_batch)?;
        }

        Ok(())
    }

    /// Ingest multiple journals.
    pub fn ingest_journals(&self, journals: &[Journal]) -> Result<usize, AnalyticsError> {
        let mut count = 0;
        for journal in journals {
            self.ingest_journal(journal)?;
            count += 1;
        }
        Ok(count)
    }

    /// Execute a SQL query and return results as formatted rows.
    pub fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, AnalyticsError> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows_iter = stmt.query(params![])?;
        let column_count = rows_iter.as_ref().unwrap().column_count();

        let mut result = Vec::new();
        while let Some(row) = rows_iter.next()? {
            let mut values = Vec::with_capacity(column_count);
            for i in 0..column_count {
                let val: String = row
                    .get::<_, duckdb::types::Value>(i)
                    .map(|v| format_value(&v))
                    .unwrap_or_else(|_| "NULL".to_string());
                values.push(val);
            }
            result.push(values);
        }
        Ok(result)
    }

    /// Execute a SQL query and return column names.
    pub fn query_columns(&self, sql: &str) -> Result<Vec<String>, AnalyticsError> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query(params![])?;
        let names = rows
            .as_ref()
            .unwrap()
            .column_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        Ok(names)
    }

    /// Get the number of experiments in the store.
    pub fn experiment_count(&self) -> Result<usize, AnalyticsError> {
        let mut stmt = self.conn.prepare("SELECT count(*) FROM experiments")?;
        let count: i64 = stmt.query_row(params![], |row| row.get(0))?;
        Ok(count as usize)
    }

    fn insert_batch(&self, table: &str, batch: &RecordBatch) -> Result<(), AnalyticsError> {
        // Use DuckDB's Arrow appender for zero-copy ingestion
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
        duckdb::types::Value::Float(f) => format!("{:.2}", f),
        duckdb::types::Value::Double(f) => format!("{:.4}", f),
        duckdb::types::Value::Text(s) => s.clone(),
        _ => format!("{:?}", v),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tumult_core::types::*;

    fn sample_journal(id: &str, status: ExperimentStatus) -> Journal {
        Journal {
            experiment_title: format!("Test {}", id),
            experiment_id: id.into(),
            status,
            started_at_ns: 1774980000000000000,
            ended_at_ns: 1774980300000000000,
            duration_ms: 300000,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![ActivityResult {
                name: "action-1".into(),
                activity_type: ActivityType::Action,
                status: ActivityStatus::Succeeded,
                started_at_ns: 1774980135000000000,
                duration_ms: 500,
                output: Some("done".into()),
                error: None,
                trace_id: "t1".into(),
                span_id: "s1".into(),
            }],
            rollback_results: vec![],
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
    fn create_in_memory_store() {
        let store = AnalyticsStore::in_memory().unwrap();
        assert_eq!(store.experiment_count().unwrap(), 0);
    }

    #[test]
    fn ingest_single_journal() {
        let store = AnalyticsStore::in_memory().unwrap();
        let journal = sample_journal("exp-001", ExperimentStatus::Completed);
        store.ingest_journal(&journal).unwrap();
        assert_eq!(store.experiment_count().unwrap(), 1);
    }

    #[test]
    fn ingest_multiple_journals() {
        let store = AnalyticsStore::in_memory().unwrap();
        let journals = vec![
            sample_journal("exp-001", ExperimentStatus::Completed),
            sample_journal("exp-002", ExperimentStatus::Deviated),
            sample_journal("exp-003", ExperimentStatus::Completed),
        ];
        let count = store.ingest_journals(&journals).unwrap();
        assert_eq!(count, 3);
        assert_eq!(store.experiment_count().unwrap(), 3);
    }

    #[test]
    fn query_experiments_by_status() {
        let store = AnalyticsStore::in_memory().unwrap();
        store
            .ingest_journal(&sample_journal("exp-001", ExperimentStatus::Completed))
            .unwrap();
        store
            .ingest_journal(&sample_journal("exp-002", ExperimentStatus::Deviated))
            .unwrap();
        store
            .ingest_journal(&sample_journal("exp-003", ExperimentStatus::Completed))
            .unwrap();

        let rows = store
            .query("SELECT experiment_id, status FROM experiments WHERE status = 'Completed'")
            .unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn query_avg_duration() {
        let store = AnalyticsStore::in_memory().unwrap();
        store
            .ingest_journal(&sample_journal("exp-001", ExperimentStatus::Completed))
            .unwrap();
        store
            .ingest_journal(&sample_journal("exp-002", ExperimentStatus::Completed))
            .unwrap();

        let rows = store
            .query("SELECT avg(duration_ms) as avg_duration FROM experiments")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "300000.0000");
    }

    #[test]
    fn query_activity_results_by_phase() {
        let store = AnalyticsStore::in_memory().unwrap();
        store
            .ingest_journal(&sample_journal("exp-001", ExperimentStatus::Completed))
            .unwrap();

        let rows = store
            .query("SELECT name, phase, duration_ms FROM activity_results WHERE phase = 'method'")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "action-1");
        assert_eq!(rows[0][1], "method");
    }

    #[test]
    fn query_resilience_score_trend() {
        let store = AnalyticsStore::in_memory().unwrap();
        store
            .ingest_journal(&sample_journal("exp-001", ExperimentStatus::Completed))
            .unwrap();
        store
            .ingest_journal(&sample_journal("exp-002", ExperimentStatus::Completed))
            .unwrap();

        let rows = store
            .query("SELECT experiment_id, resilience_score FROM experiments ORDER BY experiment_id")
            .unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn query_columns_returns_names() {
        let store = AnalyticsStore::in_memory().unwrap();
        store
            .ingest_journal(&sample_journal("exp-001", ExperimentStatus::Completed))
            .unwrap();

        let cols = store
            .query_columns("SELECT experiment_id, status, duration_ms FROM experiments")
            .unwrap();
        assert_eq!(cols, vec!["experiment_id", "status", "duration_ms"]);
    }
}
