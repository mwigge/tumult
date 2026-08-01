//! The read side of the store (split out of the crate root).

use duckdb::Connection;

use crate::error::StoreError;
use crate::rows::ExperimentRun;
use crate::store::query_json_rows;

/// The read side of the store (read-only `DuckDB` connection).
pub struct Reader {
    pub(crate) conn: Connection,
}

impl Reader {
    /// The experiment rollup view: one row per `resilience.experiment` span.
    ///
    /// # Errors
    /// Returns an error if the view cannot be queried.
    pub fn experiment_runs(&self) -> Result<Vec<ExperimentRun>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT experiment_id, experiment_name, started_at_ns, ended_at_ns,
                    duration_ns, outcome_status, hypothesis_met
             FROM experiment_runs ORDER BY started_at_ns",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ExperimentRun {
                experiment_id: r.get(0)?,
                experiment_name: r.get(1)?,
                started_at_ns: r.get(2)?,
                ended_at_ns: r.get(3)?,
                duration_ns: r.get(4)?,
                outcome_status: r.get(5)?,
                hypothesis_met: r.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Run a read-only SQL query and return each row as a JSON object
    /// (`{column: value}`). Values keep their JSON types (numbers, booleans,
    /// strings, null). Intended for the semantic-metrics layer and reports.
    ///
    /// # Errors
    /// Returns an error if the query fails to prepare or execute.
    pub fn query_json_rows(&self, sql: &str) -> Result<Vec<serde_json::Value>, StoreError> {
        query_json_rows(&self.conn, sql)
    }

    /// Raw batch execution on the read connection — crate-internal, used by
    /// the lake exporter for `COPY … TO … (FORMAT PARQUET)` (reads the store,
    /// writes only parquet files, so it is valid on a read-only connection).
    pub(crate) fn execute_batch(&self, sql: &str) -> Result<(), StoreError> {
        Ok(self.conn.execute_batch(sql)?)
    }
}
