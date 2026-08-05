//! Daemon-run storage (schema v5): the `run_registry` of validated `.toon`
//! definitions, the `runs` state machine, and the append-only `run_audit`
//! trail.
//!
//! All mutations go through [`Writer`] (the daemon's single-writer channel
//! rides them, like every other write); reads go through [`Reader`] and back
//! the `/api/runs*` endpoints and startup orphan reconciliation.

use duckdb::params;
use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::{Reader, Writer};

/// Run states (`runs.state`). The happy path is
/// `queued → validating → running → passed|deviated|failed|aborted`;
/// `stopping` is the e-stop transition, `orphaned` / `rollback_pending`
/// the crash-recovery states. T10 adds `pending_approval` as a value-level
/// addition — no schema change required.
pub mod run_state {
    pub const QUEUED: &str = "queued";
    pub const VALIDATING: &str = "validating";
    pub const RUNNING: &str = "running";
    pub const STOPPING: &str = "stopping";
    pub const PASSED: &str = "passed";
    pub const DEVIATED: &str = "deviated";
    pub const FAILED: &str = "failed";
    pub const ABORTED: &str = "aborted";
    pub const ORPHANED: &str = "orphaned";
    pub const ROLLBACK_PENDING: &str = "rollback_pending";
    /// T10: gated runs wait here until the approval quorum dispatches them.
    pub const PENDING_APPROVAL: &str = "pending_approval";
    /// T10 terminal: an approver rejected the request.
    pub const REJECTED: &str = "rejected";
    /// T10 terminal: the approval TTL lapsed before dispatch.
    pub const EXPIRED: &str = "expired";

    /// States in which a run from a previous process lifetime is an orphan:
    /// the daemon owning it is gone (only one daemon owns the store).
    /// `pending_approval` is deliberately NOT active — no execution is in
    /// flight, nothing to roll back, and the request survives the restart
    /// (an approval after restart dispatches from the stored request row).
    pub const ACTIVE: [&str; 4] = [QUEUED, VALIDATING, RUNNING, STOPPING];

    /// Terminal states (a run in one of these can never transition again).
    pub const TERMINAL: [&str; 8] = [
        PASSED,
        DEVIATED,
        FAILED,
        ABORTED,
        ORPHANED,
        ROLLBACK_PENDING,
        REJECTED,
        EXPIRED,
    ];
}

/// `runs.rollback_status` values.
pub mod rollback_status {
    pub const NOT_NEEDED: &str = "not_needed";
    pub const COMPLETED: &str = "completed";
    pub const FAILED: &str = "failed";
}

/// A validated experiment definition in `run_registry`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredDefinition {
    pub id: String,
    pub name: String,
    pub definition_toon: String,
    pub content_hash: String,
    pub registered_at_ns: i64,
    pub registered_by: Option<String>,
}

/// A run record at enqueue time (`runs`).
#[derive(Debug, Clone)]
pub struct NewRun {
    pub id: String,
    pub registry_id: String,
    pub params_json: Option<String>,
    pub queued_at_ns: i64,
    /// The authenticated identity that enqueued the run (schema v6
    /// `run_audit.actor`); `None` when unauthenticated/system.
    pub actor: Option<String>,
}

fn now_ns() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

impl Writer {
    /// Link a run to its parent campaign (`runs.gameday_id`, schema v12).
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn set_run_gameday(&self, run_id: &str, gameday_id: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE runs SET gameday_id = ? WHERE id = ?",
            params![gameday_id, run_id],
        )?;
        Ok(())
    }

    /// The active campaign parent for one gameday definition, if any
    /// (`runs.gameday_id IS NULL` in any genuinely-active state). Called on
    /// the single-writer path so `POST /api/gamedays/{id}/runs` checks and
    /// inserts atomically — two concurrent launches cannot both win.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn active_gameday_campaign(
        &self,
        registry_id: &str,
    ) -> Result<Option<serde_json::Value>, StoreError> {
        let states = run_state::ACTIVE
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(self
            .query_json_rows(&format!(
                "SELECT r.id FROM runs r \
                 WHERE r.registry_id = '{}' AND r.gameday_id IS NULL \
                   AND r.state IN ({states}) LIMIT 1",
                registry_id.replace('\'', "''")
            ))?
            .into_iter()
            .next())
    }

    /// Insert a registry definition (callers dedup by `content_hash` first).
    ///
    /// # Errors
    /// Returns an error if the row fails to insert.
    pub fn register_definition(&self, def: &RegisteredDefinition) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO run_registry \
             (id, name, definition_toon, content_hash, registered_at_ns, registered_by) \
             VALUES (?,?,?,?,?,?)",
            params![
                def.id,
                def.name,
                def.definition_toon,
                def.content_hash,
                def.registered_at_ns,
                def.registered_by
            ],
        )?;
        Ok(())
    }

    /// Register a GameDay definition (`kind = 'gameday'`): the stored
    /// `definition_toon` is the JSON envelope with the campaign TOON and the
    /// resolved experiment registry ids (see `tumult_api::gamedays`).
    ///
    /// # Errors
    /// Returns an error if the row fails to insert.
    pub fn register_gameday_definition(
        &self,
        def: &RegisteredDefinition,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO run_registry \
             (id, name, definition_toon, content_hash, registered_at_ns, registered_by, kind) \
             VALUES (?,?,?,?,?,?,'gameday')",
            params![
                def.id,
                def.name,
                def.definition_toon,
                def.content_hash,
                def.registered_at_ns,
                def.registered_by
            ],
        )?;
        Ok(())
    }

    /// Enqueue a run (state `queued`) and record the `enqueued` audit event.
    ///
    /// # Errors
    /// Returns an error if the rows fail to insert.
    pub fn insert_run(&self, run: &NewRun) -> Result<(), StoreError> {
        crate::with_tx(&self.conn, || {
            self.conn.execute(
                "INSERT INTO runs (id, registry_id, state, params_json, queued_at_ns) \
                 VALUES (?,?,?,CAST(? AS JSON),?)",
                params![
                    run.id,
                    run.registry_id,
                    run_state::QUEUED,
                    run.params_json,
                    run.queued_at_ns
                ],
            )?;
            self.insert_run_audit(&run.id, "enqueued", None, run.actor.as_deref())
        })
    }

    /// Transition a run to `state` (no timestamp side effects) and record an
    /// audit event named after the state.
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn set_run_state(&self, run_id: &str, state: &str) -> Result<(), StoreError> {
        self.set_run_state_with(run_id, state, None, None, None)
    }

    /// Like [`Self::set_run_state`] with an explicit audit event + detail
    /// (e.g. `stop_requested`, `orphan_detected`) and the authenticated
    /// identity behind the transition (`actor`; `None` for system events).
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn set_run_state_with(
        &self,
        run_id: &str,
        state: &str,
        audit_event: Option<&str>,
        audit_detail: Option<&str>,
        actor: Option<&str>,
    ) -> Result<(), StoreError> {
        crate::with_tx(&self.conn, || {
            self.conn.execute(
                "UPDATE runs SET state = ? WHERE id = ?",
                params![state, run_id],
            )?;
            self.insert_run_audit(run_id, audit_event.unwrap_or(state), audit_detail, actor)
        })
    }

    /// Mark a run `running` and stamp `started_at_ns`. `experiment_id` (the
    /// journal's id, linking the run's OTLP telemetry) is known only after
    /// execution begins, so callers pass `None` here and supply it to
    /// [`Self::finish_run`]; `COALESCE` keeps whichever arrives first.
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn mark_run_started(
        &self,
        run_id: &str,
        experiment_id: Option<&str>,
    ) -> Result<(), StoreError> {
        crate::with_tx(&self.conn, || {
            self.conn.execute(
                "UPDATE runs SET state = ?, started_at_ns = ?, \
                 experiment_id = COALESCE(?, experiment_id) WHERE id = ?",
                params![run_state::RUNNING, now_ns(), experiment_id, run_id],
            )?;
            self.insert_run_audit(run_id, "started", experiment_id, None)
        })
    }

    /// Terminally finish a run: stamps `ended_at_ns`, links the journal's
    /// `experiment_id` when supplied, records the rollback outcome and/or
    /// error, and audits the terminal state.
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn finish_run(
        &self,
        run_id: &str,
        state: &str,
        experiment_id: Option<&str>,
        rollback_status: Option<&str>,
        error: Option<&str>,
    ) -> Result<(), StoreError> {
        crate::with_tx(&self.conn, || {
            self.conn.execute(
                "UPDATE runs SET state = ?, ended_at_ns = ?, \
                 experiment_id = COALESCE(?, experiment_id), rollback_status = ?, error = ? \
                 WHERE id = ?",
                params![
                    state,
                    now_ns(),
                    experiment_id,
                    rollback_status,
                    error,
                    run_id
                ],
            )?;
            self.insert_run_audit(run_id, state, error, None)
        })
    }

    /// Append one event to a run's audit trail. `actor` is the authenticated
    /// session identity behind the event (schema v6); `None` for system
    /// events. Every event is a link in the run's hash chain (schema v7):
    /// `new_hash` covers the event content plus the previous link's hash, so
    /// rewriting history breaks the chain — see
    /// [`Reader::verify_run_audit_chain`](crate::Reader::verify_run_audit_chain).
    ///
    /// # Errors
    /// Returns an error if the row fails to insert.
    pub fn insert_run_audit(
        &self,
        run_id: &str,
        event: &str,
        detail: Option<&str>,
        actor: Option<&str>,
    ) -> Result<(), StoreError> {
        let at_ns = now_ns();
        let prev_hash: Option<String> = self
            .conn
            .prepare(&format!(
                "SELECT new_hash FROM run_audit WHERE run_id = '{}' \
                 ORDER BY at_ns DESC, rowid DESC LIMIT 1",
                run_id.replace('\'', "''")
            ))
            .and_then(|mut stmt| stmt.query_row(params![], |row| row.get(0)))
            .ok();
        let new_hash = crate::approvals::audit_chain_hash(
            run_id,
            at_ns,
            event,
            detail,
            actor,
            prev_hash.as_deref(),
        );
        self.conn.execute(
            "INSERT INTO run_audit (run_id, at_ns, event, detail, actor, prev_hash, new_hash) \
             VALUES (?,?,?,?,?,?,?)",
            params![run_id, at_ns, event, detail, actor, prev_hash, new_hash],
        )?;
        Ok(())
    }
}

impl Reader {
    /// Fetch a registry definition by id (includes the `.toon` source).
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn registry_definition(
        &self,
        id: &str,
    ) -> Result<Option<RegisteredDefinition>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT * FROM run_registry WHERE id = '{}'",
            id.replace('\'', "''")
        ))?;
        Ok(rows.first().map(row_to_definition))
    }

    /// Fetch a registry definition by content hash (registration dedup).
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn registry_by_hash(
        &self,
        content_hash: &str,
    ) -> Result<Option<RegisteredDefinition>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT * FROM run_registry WHERE content_hash = '{}' ORDER BY registered_at_ns \
             LIMIT 1",
            content_hash.replace('\'', "''")
        ))?;
        Ok(rows.first().map(row_to_definition))
    }

    /// List registered definitions, newest first (metadata only — the UI's
    /// registry picker; the `.toon` source comes from
    /// [`Self::registry_definition`]).
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn registry_list(&self, limit: u32) -> Result<Vec<serde_json::Value>, StoreError> {
        self.query_json_rows(&format!(
            "SELECT id, name, content_hash, registered_at_ns, registered_by \
             FROM run_registry ORDER BY registered_at_ns DESC LIMIT {limit}"
        ))
    }

    /// List runs, newest first; `state` filters to one state.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn runs(
        &self,
        state: Option<&str>,
        limit: u32,
    ) -> Result<Vec<serde_json::Value>, StoreError> {
        let filter = state.map_or_else(String::new, |s| {
            format!("WHERE r.state = '{}'", s.replace('\'', "''"))
        });
        self.query_json_rows(&format!(
            "SELECT r.*, g.name AS definition_name FROM runs r \
             LEFT JOIN run_registry g ON g.id = r.registry_id \
             {filter} ORDER BY r.queued_at_ns DESC LIMIT {limit}"
        ))
    }

    /// One run by id (joined with its definition name), or `None`.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn run_get(&self, id: &str) -> Result<Option<serde_json::Value>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT r.*, g.name AS definition_name FROM runs r \
             LEFT JOIN run_registry g ON g.id = r.registry_id \
             WHERE r.id = '{}'",
            id.replace('\'', "''")
        ))?;
        Ok(rows.into_iter().next())
    }

    /// A run's audit trail, oldest first.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn run_audit_trail(&self, id: &str) -> Result<Vec<serde_json::Value>, StoreError> {
        self.query_json_rows(&format!(
            "SELECT * FROM run_audit WHERE run_id = '{}' ORDER BY at_ns",
            id.replace('\'', "''")
        ))
    }

    /// Active-state runs joined with their definitions — the orphan
    /// reconciliation input at daemon startup. GameDay campaign parents
    /// (`run_registry.kind = 'gameday'`) are excluded: they own no fault
    /// execution, so the orphan sweep's rollback would be nonsense — the
    /// gameday supervisor resumes a recovered `queued`/`running` parent on
    /// its next tick.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn active_runs(&self) -> Result<Vec<serde_json::Value>, StoreError> {
        let states = run_state::ACTIVE
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(", ");
        self.query_json_rows(&format!(
            "SELECT r.*, g.name AS definition_name, g.definition_toon FROM runs r \
             JOIN run_registry g ON g.id = r.registry_id \
             WHERE r.state IN ({states}) AND g.kind IS DISTINCT FROM 'gameday' \
             ORDER BY r.queued_at_ns"
        ))
    }
}

/// Map a `run_registry` JSON row to the typed definition.
fn row_to_definition(v: &serde_json::Value) -> RegisteredDefinition {
    RegisteredDefinition {
        id: v["id"].as_str().unwrap_or_default().to_string(),
        name: v["name"].as_str().unwrap_or_default().to_string(),
        definition_toon: v["definition_toon"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        content_hash: v["content_hash"].as_str().unwrap_or_default().to_string(),
        registered_at_ns: v["registered_at_ns"].as_i64().unwrap_or(0),
        registered_by: v["registered_by"].as_str().map(str::to_string),
    }
}
