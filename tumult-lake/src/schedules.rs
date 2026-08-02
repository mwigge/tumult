//! Recurring-run storage (schema v10): the `run_schedules` table — one row
//! per interval schedule, created via the API and fired by the daemon's
//! schedule scheduler. Same additive, index-free rule as the other v5+
//! tables: the table is tiny, scans are free, and uniqueness (`id`) is
//! enforced in code.
//!
//! Schedules are interval-based (`interval_s`), not cron expressions — the
//! workspace has no cron parser and one is not worth a dependency for the
//! first iteration; a future `cron` column can sit alongside without
//! disturbing this schema.

use duckdb::params;
use serde_json::Value;

use crate::error::StoreError;
use crate::{Reader, Writer};

/// One `run_schedules` row.
#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleRow {
    pub id: String,
    pub name: String,
    pub registry_id: String,
    /// Fire interval in seconds; the next fire is `now + interval` after
    /// each fire, so missed fires during downtime collapse into one.
    pub interval_s: i64,
    /// Template variables for the definition (same shape as
    /// `runs.params_json`); `None` for none.
    pub vars_json: Option<String>,
    /// Execution environment, tier-classified at fire time like
    /// `POST /api/runs` (`"dev"` default).
    pub env: String,
    pub target: Option<String>,
    pub enabled: bool,
    pub next_run_at_ns: i64,
    pub last_run_at_ns: Option<i64>,
    pub last_run_id: Option<String>,
    pub created_by: Option<String>,
    pub created_at_ns: i64,
}

impl Writer {
    /// Insert a schedule row.
    ///
    /// # Errors
    /// Returns an error if the insert fails.
    pub fn create_schedule(&self, s: &ScheduleRow) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO run_schedules VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
            params![
                s.id,
                s.name,
                s.registry_id,
                s.interval_s,
                s.vars_json,
                s.env,
                s.target,
                s.enabled,
                s.next_run_at_ns,
                s.last_run_at_ns,
                s.last_run_id,
                s.created_by,
                s.created_at_ns
            ],
        )?;
        Ok(())
    }

    /// Enable or disable a schedule.
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn set_schedule_enabled(&self, id: &str, enabled: bool) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE run_schedules SET enabled = ? WHERE id = ?",
            params![enabled, id],
        )?;
        Ok(())
    }

    /// Delete a schedule by id.
    ///
    /// # Errors
    /// Returns an error if the delete fails.
    pub fn delete_schedule(&self, id: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM run_schedules WHERE id = ?", params![id])?;
        Ok(())
    }

    /// Record a fire: stamp the run it produced and advance `next_run_at_ns`.
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn schedule_fired(
        &self,
        id: &str,
        run_id: Option<&str>,
        fired_at_ns: i64,
        next_run_at_ns: i64,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE run_schedules \
             SET last_run_at_ns = ?, last_run_id = ?, next_run_at_ns = ? WHERE id = ?",
            params![fired_at_ns, run_id, next_run_at_ns, id],
        )?;
        Ok(())
    }
}

impl Reader {
    /// List all schedules, ordered by name.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn list_schedules(&self) -> Result<Vec<ScheduleRow>, StoreError> {
        let rows = self.query_json_rows("SELECT * FROM run_schedules ORDER BY name")?;
        Ok(rows.iter().map(row_to_schedule).collect())
    }

    /// List enabled schedules due at or before `now_ns`, oldest first.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn due_schedules(&self, now_ns: i64) -> Result<Vec<ScheduleRow>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT * FROM run_schedules WHERE enabled AND next_run_at_ns <= {now_ns} \
             ORDER BY next_run_at_ns"
        ))?;
        Ok(rows.iter().map(row_to_schedule).collect())
    }
}

fn row_to_schedule(v: &Value) -> ScheduleRow {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    ScheduleRow {
        id: s("id"),
        name: s("name"),
        registry_id: s("registry_id"),
        interval_s: v["interval_s"].as_i64().unwrap_or(0),
        vars_json: v["vars_json"].as_str().map(str::to_string),
        env: s("env"),
        target: v["target"].as_str().map(str::to_string),
        enabled: v["enabled"].as_bool().unwrap_or(false),
        next_run_at_ns: v["next_run_at_ns"].as_i64().unwrap_or(0),
        last_run_at_ns: v["last_run_at_ns"].as_i64(),
        last_run_id: v["last_run_id"].as_str().map(str::to_string),
        created_by: v["created_by"].as_str().map(str::to_string),
        created_at_ns: v["created_at_ns"].as_i64().unwrap_or(0),
    }
}
