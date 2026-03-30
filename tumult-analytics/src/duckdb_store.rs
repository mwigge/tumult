//! DuckDB embedded analytics store.

use arrow::record_batch::RecordBatch;
use duckdb::{params, Connection};
use std::path::Path;
use tumult_core::types::Journal;

use crate::arrow_convert::{journal_to_activity_batch, journal_to_experiment_batch};
use crate::error::AnalyticsError;

pub struct AnalyticsStore {
    conn: Connection,
}

impl AnalyticsStore {
    pub fn in_memory() -> Result<Self, AnalyticsError> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.create_tables()?;
        Ok(store)
    }

    pub fn open(path: &Path) -> Result<Self, AnalyticsError> {
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.create_tables()?;
        Ok(store)
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
            CREATE TABLE IF NOT EXISTS activity_results (
                experiment_id VARCHAR NOT NULL, name VARCHAR NOT NULL,
                activity_type VARCHAR NOT NULL, status VARCHAR NOT NULL,
                started_at_ns BIGINT NOT NULL, duration_ms UBIGINT NOT NULL,
                output VARCHAR, error VARCHAR, phase VARCHAR NOT NULL
            );",
        )?;
        Ok(())
    }

    /// Ingest a single experiment journal into the analytics store.
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
    /// };
    ///
    /// store.ingest_journal(&journal).unwrap();
    /// assert_eq!(store.experiment_count().unwrap(), 1);
    /// ```
    pub fn ingest_journal(&self, journal: &Journal) -> Result<(), AnalyticsError> {
        let exp_batch = journal_to_experiment_batch(journal)?;
        let act_batch = journal_to_activity_batch(journal)?;
        self.insert_batch("experiments", &exp_batch)?;
        if act_batch.num_rows() > 0 {
            self.insert_batch("activity_results", &act_batch)?;
        }
        Ok(())
    }

    pub fn ingest_journals(&self, journals: &[Journal]) -> Result<usize, AnalyticsError> {
        let mut count = 0;
        for journal in journals {
            self.ingest_journal(journal)?;
            count += 1;
        }
        Ok(count)
    }

    pub fn query(&self, sql: &str) -> Result<Vec<Vec<String>>, AnalyticsError> {
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows_iter = stmt.query(params![])?;
        let column_count = rows_iter.as_ref().map(|r| r.column_count()).unwrap_or(0);
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

    pub fn query_columns(&self, sql: &str) -> Result<Vec<String>, AnalyticsError> {
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query(params![])?;
        let names = rows
            .as_ref()
            .map(|r| r.column_names())
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        Ok(names)
    }

    pub fn experiment_count(&self) -> Result<usize, AnalyticsError> {
        let mut stmt = self.conn.prepare("SELECT count(*) FROM experiments")?;
        let count: i64 = stmt.query_row(params![], |row| row.get(0))?;
        Ok(count as usize)
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
            .query("SELECT experiment_id FROM experiments WHERE status = 'Completed'")
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
}
