//! Import rows from legacy pre-unification databases (the old
//! `tumult-analytics` store and/or the kronika lake) into this store.
//!
//! The source is `ATTACH`ed read-only and each known table is merged with an
//! anti-join on its natural key, so re-running the import never duplicates
//! rows. Column sets are resolved per source through `information_schema`
//! and intersected with the target's, so older schemas missing later columns
//! (e.g. kronika v1 `metric_histograms` without the promoted dimension
//! columns) still import — absent columns land as `NULL`.

use std::path::Path;

use duckdb::params;

use crate::error::AnalyticsError;

use super::AnalyticsStore;

/// Target tables and the natural key used to skip rows already present.
/// `IS NOT DISTINCT FROM` semantics make `NULL` key parts compare equal, so
/// re-imports are idempotent even for tables with nullable keys (`logs`).
const LEGACY_TABLES: &[(&str, &[&str])] = &[
    ("experiments", &["experiment_id"]),
    (
        "activity_results",
        &["experiment_id", "name", "started_at_ns"],
    ),
    ("load_results", &["experiment_id", "tool", "started_at_ns"]),
    ("agentic_runs", &["run_id"]),
    (
        "agentic_contract_outcomes",
        &["run_id", "contract_type", "scenario"],
    ),
    (
        "agentic_fault_applications",
        &["run_id", "fault_type", "scenario"],
    ),
    ("agentic_replay_outcomes", &["run_id", "replay_id"]),
    ("graph_nodes", &["id"]),
    ("graph_edges", &["src", "rel", "dst", "run_id", "ts"]),
    ("autopilot_decisions", &["id"]),
    ("autopilot_events", &["decision_id", "at_ns", "kind"]),
    (
        "autopilot_change_events",
        &["service_id", "at_ns", "source"],
    ),
    ("spans", &["trace_id", "span_id"]),
    ("logs", &["ts_ns", "trace_id", "span_id", "body"]),
    ("metric_sums", &["ts_ns", "metric_name"]),
    ("metric_gauges", &["ts_ns", "metric_name"]),
    ("metric_histograms", &["ts_ns", "metric_name"]),
    ("manual_experiments", &["id"]),
    ("manual_experiment_audit", &["id"]),
    ("evidence_attachments", &["id"]),
    ("import_batches", &["id"]),
];

/// Attach alias for the legacy source being merged.
const LEGACY_ALIAS: &str = "legacy";

impl AnalyticsStore {
    /// Merge every known table from the legacy database at `source` into
    /// this store, returning `(table, rows_inserted)` per table present in
    /// the source. Idempotent: rows whose natural key already exists are
    /// skipped, so a re-run reports zeros.
    ///
    /// # Errors
    ///
    /// Returns an error if the source file is missing, cannot be attached
    /// read-only, or a merge statement fails.
    pub fn import_legacy(&self, source: &Path) -> Result<Vec<(String, usize)>, AnalyticsError> {
        if !source.exists() {
            return Err(AnalyticsError::Internal(format!(
                "legacy store not found: {}",
                source.display()
            )));
        }
        let escaped = source.display().to_string().replace('\'', "''");
        self.conn
            .execute_batch(&format!("ATTACH '{escaped}' AS {LEGACY_ALIAS} (READ_ONLY)"))?;
        let result = self.merge_legacy_tables();
        let _ = self.conn.execute_batch(&format!("DETACH {LEGACY_ALIAS}"));
        result
    }

    /// Merge each known table from the attached `legacy` catalog.
    fn merge_legacy_tables(&self) -> Result<Vec<(String, usize)>, AnalyticsError> {
        let current: String = self
            .conn
            .query_row("SELECT current_database()", [], |r| r.get(0))?;
        let mut report = Vec::new();
        for (table, key) in LEGACY_TABLES {
            let src_cols = self.table_columns(LEGACY_ALIAS, table)?;
            if src_cols.is_empty() {
                continue; // table absent in this source
            }
            let dst_cols = self.table_columns(&current, table)?;
            let cols: Vec<&String> = dst_cols.iter().filter(|c| src_cols.contains(c)).collect();
            // A source missing part of the natural key cannot be deduped
            // safely — skip it rather than risk duplicate rows.
            if key.iter().any(|k| !cols.iter().any(|c| c.as_str() == *k)) {
                continue;
            }
            let col_list = cols
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            let predicate = key
                .iter()
                .map(|k| format!("cur.{k} IS NOT DISTINCT FROM src.{k}"))
                .collect::<Vec<_>>()
                .join(" AND ");
            let sql = format!(
                "INSERT INTO {table} ({col_list})
                 SELECT {col_list} FROM {LEGACY_ALIAS}.{table} src
                 WHERE NOT EXISTS (SELECT 1 FROM {table} cur WHERE {predicate})"
            );
            let inserted = self.conn.execute(&sql, [])?;
            report.push(((*table).to_string(), inserted));
        }
        Ok(report)
    }

    /// Column names of `table` in `catalog`, in declaration order.
    fn table_columns(&self, catalog: &str, table: &str) -> Result<Vec<String>, AnalyticsError> {
        let mut stmt = self.conn.prepare(
            "SELECT column_name FROM information_schema.columns
             WHERE table_catalog = ? AND table_name = ? ORDER BY ordinal_position",
        )?;
        let cols = stmt
            .query_map(params![catalog, table], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(cols)
    }
}
