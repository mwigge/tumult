//! The statements of the runs, their registry and their audit trail
//! (`runs.rs`).

use crate::run_state;

pub(crate) const INSERT_RUN: &str =
    "INSERT INTO runs (id, registry_id, state, params_json, queued_at_ns) \
    VALUES (?,?,?,CAST(? AS JSON),?)";

pub(crate) const SET_GAMEDAY: &str = "UPDATE runs SET gameday_id = ? WHERE id = ?";

pub(crate) const INSERT_DEFINITION: &str = "INSERT INTO run_registry \
    (id, name, definition_toon, content_hash, registered_at_ns, registered_by) \
    VALUES (?,?,?,?,?,?)";

pub(crate) const INSERT_GAMEDAY_DEFINITION: &str = "INSERT INTO run_registry \
    (id, name, definition_toon, content_hash, registered_at_ns, registered_by, kind) \
    VALUES (?,?,?,?,?,?,'gameday')";

pub(crate) const INSERT_EXECUTION_PIN: &str =
    "INSERT INTO run_execution_pins (run_id, execution_hash) VALUES (?, ?)";

pub(crate) const SET_STATE: &str = "UPDATE runs SET state = ? WHERE id = ? AND state NOT IN ('passed','deviated','failed','aborted','rejected','expired') AND (? <> 'queued' OR state = 'pending_approval')";

pub(crate) const MARK_STARTED: &str = "UPDATE runs SET state = ?, started_at_ns = ?, \
    experiment_id = COALESCE(?, experiment_id) WHERE id = ? AND state IN ('queued', 'validating')";

pub(crate) const CLAIM: &str =
    "UPDATE runs SET state = 'validating' WHERE id = ? AND state = 'queued'";

pub(crate) const BEGIN_EXECUTION: &str =
    "UPDATE runs SET state = 'running', started_at_ns = ? WHERE id = ? AND state = 'validating'";

pub(crate) const CONSUME_PINNED_APPROVAL: &str = "UPDATE approval_requests SET consumed_at_ns = ? WHERE run_id = ? AND pin_hash = ? AND consumed_at_ns IS NULL AND (break_glass = TRUE OR expires_at_ns >= ?)";

pub(crate) const INSERT_RECOVERY_PLAN: &str =
    "INSERT INTO run_recovery_plans (run_id, plan_json) VALUES (?, CAST(? AS JSON))";

pub(crate) const REQUEST_STOP: &str = "UPDATE runs SET state = CASE WHEN started_at_ns IS NULL THEN 'aborted' ELSE 'stopping' END, ended_at_ns = CASE WHEN started_at_ns IS NULL THEN ? ELSE ended_at_ns END, rollback_status = CASE WHEN started_at_ns IS NULL THEN 'not_needed' ELSE rollback_status END, error = CASE WHEN started_at_ns IS NULL THEN 'cancelled before start' ELSE error END WHERE id = ? AND state IN ('queued','validating','running','stopping','pending_approval')";

pub(crate) const STATE: &str = "SELECT state FROM runs WHERE id = ?";

pub(crate) const FAIL: &str = "UPDATE runs SET state = CASE WHEN started_at_ns IS NULL THEN 'failed' ELSE 'rollback_pending' END, error = ?, ended_at_ns = CASE WHEN started_at_ns IS NULL THEN ? ELSE ended_at_ns END WHERE id = ? AND state IN ('queued','validating','running','stopping')";

pub(crate) const FINISH: &str = "UPDATE runs SET state = ?, ended_at_ns = ?, \
    experiment_id = COALESCE(?, experiment_id), rollback_status = ?, error = ? \
    WHERE id = ? AND (NOT ? OR state = 'pending_approval')";

pub(crate) const LAST_AUDIT_HASH: &str =
    "SELECT new_hash FROM run_audit WHERE run_id = ? ORDER BY at_ns DESC, rowid DESC LIMIT 1";

pub(crate) const INSERT_AUDIT: &str =
    "INSERT INTO run_audit (run_id, at_ns, event, detail, actor, prev_hash, new_hash) \
    VALUES (?,?,?,?,?,?,?)";

pub(crate) const EXECUTION_HASH: &str =
    "SELECT execution_hash FROM run_execution_pins WHERE run_id = ?";

/// The active states of a run, as the list of an `IN`.
fn active_states() -> String {
    run_state::ACTIVE
        .iter()
        .map(|s| format!("'{s}'"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The active campaign of the gameday definition `registry_id`.
pub(crate) fn active_campaign(registry_id: &str) -> String {
    let states = active_states();
    format!(
        "SELECT r.id FROM runs r \
         WHERE r.registry_id = '{}' AND r.gameday_id IS NULL \
           AND r.state IN ({states}) LIMIT 1",
        registry_id.replace('\'', "''")
    )
}

/// The registry definition `id`.
pub(crate) fn definition(id: &str) -> String {
    format!(
        "SELECT * FROM run_registry WHERE id = '{}'",
        id.replace('\'', "''")
    )
}

/// The first registry definition with the content hash `content_hash`.
pub(crate) fn definition_by_hash(content_hash: &str) -> String {
    format!(
        "SELECT * FROM run_registry WHERE content_hash = '{}' ORDER BY registered_at_ns \
         LIMIT 1",
        content_hash.replace('\'', "''")
    )
}

/// The registry definitions, newest first, without their source.
pub(crate) fn definitions(limit: u32) -> String {
    format!(
        "SELECT id, name, content_hash, registered_at_ns, registered_by \
         FROM run_registry ORDER BY registered_at_ns DESC LIMIT {limit}"
    )
}

/// The runs, newest first, in the state `state` when there is one.
pub(crate) fn runs(state: Option<&str>, limit: u32) -> String {
    let filter = state.map_or_else(String::new, |s| {
        format!("WHERE r.state = '{}'", s.replace('\'', "''"))
    });
    format!(
        "SELECT r.*, g.name AS definition_name FROM runs r \
         LEFT JOIN run_registry g ON g.id = r.registry_id \
         {filter} ORDER BY r.queued_at_ns DESC LIMIT {limit}"
    )
}

/// The run `id` with the name of its definition.
pub(crate) fn run(id: &str) -> String {
    format!(
        "SELECT r.*, g.name AS definition_name FROM runs r \
         LEFT JOIN run_registry g ON g.id = r.registry_id \
         WHERE r.id = '{}'",
        id.replace('\'', "''")
    )
}

/// The audit trail of the run `id`, oldest first.
pub(crate) fn audit_trail(id: &str) -> String {
    format!(
        "SELECT * FROM run_audit WHERE run_id = '{}' ORDER BY at_ns",
        id.replace('\'', "''")
    )
}

/// The active runs with their definitions, but for gameday campaigns.
pub(crate) fn active_runs() -> String {
    let states = active_states();
    format!(
        "SELECT r.*, g.name AS definition_name, g.definition_toon, p.plan_json AS recovery_plan_json FROM runs r \
         JOIN run_registry g ON g.id = r.registry_id \
         LEFT JOIN run_recovery_plans p ON p.run_id = r.id \
         WHERE r.state IN ({states}) AND g.kind IS DISTINCT FROM 'gameday' \
         ORDER BY r.queued_at_ns"
    )
}

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        INSERT_RUN,
        SET_GAMEDAY,
        INSERT_DEFINITION,
        INSERT_GAMEDAY_DEFINITION,
        INSERT_EXECUTION_PIN,
        SET_STATE,
        MARK_STARTED,
        CLAIM,
        BEGIN_EXECUTION,
        CONSUME_PINNED_APPROVAL,
        INSERT_RECOVERY_PLAN,
        REQUEST_STOP,
        STATE,
        FAIL,
        FINISH,
        LAST_AUDIT_HASH,
        INSERT_AUDIT,
        EXECUTION_HASH,
        active_campaign("reg-1"),
        definition("reg-1"),
        definition_by_hash("sha256:1"),
        definitions(50),
        runs(Some("queued"), 50),
        runs(None, 50),
        run("run-1"),
        audit_trail("run-1"),
        active_runs()
    ]
}
