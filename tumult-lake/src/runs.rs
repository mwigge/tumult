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
    /// Insert a registry definition (callers dedup by `content_hash` first).
    ///
    /// # Errors
    /// Returns an error if the row fails to insert.
    pub fn register_definition(&self, def: &RegisteredDefinition) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO run_registry VALUES (?,?,?,?,?,?)",
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
    /// reconciliation input at daemon startup.
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
             WHERE r.state IN ({states}) ORDER BY r.queued_at_ns"
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    fn fixture() -> (tempfile::TempDir, crate::Store) {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        (d, store)
    }

    fn def(id: &str, hash: &str) -> RegisteredDefinition {
        RegisteredDefinition {
            id: id.into(),
            name: "latency drill".into(),
            definition_toon: "title: latency drill".into(),
            content_hash: hash.into(),
            registered_at_ns: 1,
            registered_by: Some("test".into()),
        }
    }

    #[test]
    fn registry_roundtrip_and_hash_dedup_lookup() {
        let (_d, store) = fixture();
        let writer = store.writer().unwrap();
        writer.register_definition(&def("reg-1", "hash-1")).unwrap();

        let reader = store.read_only().unwrap();
        let by_id = reader.registry_definition("reg-1").unwrap().unwrap();
        assert_eq!(by_id.definition_toon, "title: latency drill");
        let by_hash = reader.registry_by_hash("hash-1").unwrap().unwrap();
        assert_eq!(by_hash.id, "reg-1");
        assert!(reader.registry_by_hash("nope").unwrap().is_none());
    }

    #[test]
    fn run_state_machine_and_audit_trail() {
        let (_d, store) = fixture();
        let writer = store.writer().unwrap();
        writer.register_definition(&def("reg-1", "hash-1")).unwrap();
        writer
            .insert_run(&NewRun {
                id: "run-1".into(),
                registry_id: "reg-1".into(),
                params_json: Some(r#"{"env":"staging"}"#.into()),
                queued_at_ns: 10,
                actor: Some("alice".into()),
            })
            .unwrap();
        writer
            .set_run_state("run-1", run_state::VALIDATING)
            .unwrap();
        writer.mark_run_started("run-1", None).unwrap();
        writer
            .set_run_state_with(
                "run-1",
                run_state::STOPPING,
                Some("stop_requested"),
                None,
                Some("alice"),
            )
            .unwrap();
        writer
            .finish_run(
                "run-1",
                run_state::ABORTED,
                Some("exp-1"),
                Some(rollback_status::COMPLETED),
                None,
            )
            .unwrap();

        let reader = store.read_only().unwrap();
        let run = reader.run_get("run-1").unwrap().unwrap();
        assert_eq!(run["state"], serde_json::json!("aborted"));
        assert_eq!(run["experiment_id"], serde_json::json!("exp-1"));
        assert_eq!(run["definition_name"], serde_json::json!("latency drill"));
        assert_eq!(
            run["rollback_status"],
            serde_json::json!(rollback_status::COMPLETED)
        );
        assert!(run["started_at_ns"].as_i64().unwrap() > 0);
        assert!(run["ended_at_ns"].as_i64().unwrap() > 0);

        let audit = reader.run_audit_trail("run-1").unwrap();
        let events: Vec<&str> = audit.iter().filter_map(|e| e["event"].as_str()).collect();
        assert_eq!(
            events,
            [
                "enqueued",
                "validating",
                "started",
                "stop_requested",
                "aborted"
            ]
        );
        // The user-initiated transitions carry the actor; system events don't.
        let by_event = |e: &str| audit.iter().find(|r| r["event"] == e).unwrap();
        assert_eq!(by_event("enqueued")["actor"], serde_json::json!("alice"));
        assert_eq!(
            by_event("stop_requested")["actor"],
            serde_json::json!("alice")
        );
        assert!(by_event("started")["actor"].is_null());

        // Active listing is empty for a terminal run; runs() lists it.
        assert!(reader.active_runs().unwrap().is_empty());
        assert_eq!(reader.runs(None, 10).unwrap().len(), 1);
        assert_eq!(reader.runs(Some(run_state::ABORTED), 10).unwrap().len(), 1);
        assert!(reader
            .runs(Some(run_state::RUNNING), 10)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn active_runs_joins_definition_for_reconciliation() {
        let (_d, store) = fixture();
        let writer = store.writer().unwrap();
        writer.register_definition(&def("reg-1", "hash-1")).unwrap();
        writer
            .insert_run(&NewRun {
                id: "run-9".into(),
                registry_id: "reg-1".into(),
                params_json: None,
                queued_at_ns: 5,
                actor: None,
            })
            .unwrap();
        writer.mark_run_started("run-9", Some("exp-9")).unwrap();

        let reader = store.read_only().unwrap();
        let active = reader.active_runs().unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0]["id"], serde_json::json!("run-9"));
        assert_eq!(active[0]["state"], serde_json::json!("running"));
        assert_eq!(
            active[0]["definition_toon"],
            serde_json::json!("title: latency drill")
        );
    }

    /// The v4 run tables (primary keys + secondary indexes) as a raw DDL
    /// fixture, so the v4 → v5 rebuild can be exercised end to end.
    const V4_RUN_TABLE_DDL: &str = "
CREATE TABLE schema_meta (key VARCHAR PRIMARY KEY, value BIGINT NOT NULL);
INSERT INTO schema_meta (key, value) VALUES ('version', 4);
CREATE TABLE run_registry (
    id               VARCHAR PRIMARY KEY,
    name             VARCHAR NOT NULL,
    definition_toon  VARCHAR NOT NULL,
    content_hash     VARCHAR NOT NULL,
    registered_at_ns BIGINT NOT NULL,
    registered_by    VARCHAR
);
CREATE INDEX idx_run_registry_hash ON run_registry (content_hash);
CREATE TABLE runs (
    id              VARCHAR PRIMARY KEY,
    registry_id     VARCHAR NOT NULL,
    state           VARCHAR NOT NULL,
    params_json     JSON,
    experiment_id   VARCHAR,
    rollback_status VARCHAR,
    error           VARCHAR,
    queued_at_ns    BIGINT NOT NULL,
    started_at_ns   BIGINT,
    ended_at_ns     BIGINT
);
CREATE INDEX idx_runs_state ON runs (state);
CREATE INDEX idx_runs_registry ON runs (registry_id);
CREATE TABLE run_audit (
    run_id  VARCHAR NOT NULL,
    at_ns   BIGINT NOT NULL,
    event   VARCHAR NOT NULL,
    detail  VARCHAR
);
CREATE INDEX idx_run_audit_run ON run_audit (run_id, at_ns);
";

    #[test]
    fn v4_store_migrates_to_index_free_run_tables() {
        let d = tempfile::TempDir::new().unwrap();
        let db = d.path().join("kronika.duckdb");
        // Build a v4-era store with a raw connection (Store::open would
        // migrate immediately).
        {
            let conn = duckdb::Connection::open(&db).unwrap();
            conn.execute_batch(V4_RUN_TABLE_DDL).unwrap();
            conn.execute_batch(
                "INSERT INTO run_registry VALUES ('reg-old','old exp','title: old','h',1,NULL);
                 INSERT INTO runs (id, registry_id, state, queued_at_ns) \
                 VALUES ('run-old', 'reg-old', 'running', 1);
                 INSERT INTO run_audit VALUES ('run-old', 1, 'enqueued', NULL);",
            )
            .unwrap();
        }

        let store = Store::open(&db).unwrap();
        let writer = store.writer().unwrap();
        assert_eq!(
            writer.schema_version().unwrap(),
            crate::CURRENT_SCHEMA_VERSION
        );

        // Data survived the rebuild…
        let reader = store.read_only().unwrap();
        let run = reader.run_get("run-old").unwrap().unwrap();
        assert_eq!(run["state"], serde_json::json!("running"));
        assert_eq!(
            reader.registry_definition("reg-old").unwrap().unwrap().name,
            "old exp"
        );
        assert_eq!(reader.run_audit_trail("run-old").unwrap().len(), 1);

        // …and — the whole point — UPDATEs work without any ART index.
        writer
            .set_run_state("run-old", run_state::ORPHANED)
            .unwrap();
        let reader = store.read_only().unwrap();
        let run = reader.run_get("run-old").unwrap().unwrap();
        assert_eq!(run["state"], serde_json::json!("orphaned"));

        // No indexes remain on the run tables.
        let index_rows = reader
            .query_json_rows(
                "SELECT index_name, table_name FROM duckdb_indexes() \
                 WHERE table_name IN ('runs', 'run_registry', 'run_audit')",
            )
            .unwrap();
        assert!(index_rows.is_empty(), "{index_rows:?}");

        // Re-opening is a no-op (version already current, no rebuild attempted).
        drop(store);
        let store = Store::open(&db).unwrap();
        let writer = store.writer().unwrap();
        assert_eq!(
            writer.schema_version().unwrap(),
            crate::CURRENT_SCHEMA_VERSION
        );
        let reader = store.read_only().unwrap();
        assert!(reader.run_get("run-old").unwrap().is_some());
    }
}
