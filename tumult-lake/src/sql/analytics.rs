//! The statements of the analytics store: journals, agentic runs, autopilot
//! decisions and the export of their tables (`duckdb_store/`).

pub(crate) const EXPERIMENT_EXISTS: &str =
    "SELECT count(*) FROM experiments WHERE experiment_id = ?";

pub(crate) const AGENTIC_RUN_EXISTS: &str = "SELECT count(*) FROM agentic_runs WHERE run_id = ?";

pub(crate) const INSERT_AGENTIC_RUN: &str = "INSERT INTO agentic_runs (
        run_id, experiment_id, target_type, scenario,
        resilience_score, trace_id, replay_id
    ) VALUES (?, ?, ?, ?, ?, ?, ?)";

pub(crate) const INSERT_CONTRACT_OUTCOME: &str = "INSERT INTO agentic_contract_outcomes (
        run_id, scenario, contract_type, passed, reason, severity
    ) VALUES (?, ?, ?, ?, ?, ?)";

pub(crate) const INSERT_FAULT_APPLICATION: &str = "INSERT INTO agentic_fault_applications (
        run_id, scenario, fault_type, applied
    ) VALUES (?, ?, ?, ?)";

pub(crate) const INSERT_REPLAY_OUTCOME: &str = "INSERT INTO agentic_replay_outcomes (
        run_id, replay_id, scenario, passed
    ) VALUES (?, ?, ?, ?)";

pub(crate) const PURGE_EXPERIMENTS: &str =
    "DELETE FROM experiments WHERE started_at_ns < ? RETURNING experiment_id";

pub(crate) const ACTIVITIES_EXPORT: &str =
    "SELECT experiment_id, name, activity_type, status, started_at_ns, \
    duration_ms, output, error, phase FROM activity_results";

pub(crate) const DENY_EXTERNAL_ACCESS: &str = "SET enable_external_access = false";

pub(crate) const INSERT_AUTOPILOT_DECISION: &str = "INSERT INTO autopilot_decisions VALUES
    (?, ?, ?, ?, ?, ?, ?, ?, ?, CAST(? AS JSON), ?, ?, CAST(? AS JSON), ?,
     CAST(? AS JSON), CAST(? AS JSON), ?, ?)";

pub(crate) const INSERT_AUTOPILOT_EVENT: &str =
    "INSERT INTO autopilot_events VALUES (?, ?, ?, CAST(? AS JSON))";

pub(crate) const INSERT_CHANGE_EVENT: &str =
    "INSERT INTO autopilot_change_events VALUES (?, ?, ?, ?)";

pub(crate) const CURRENT_DATABASE: &str = "SELECT current_database()";

pub(crate) const TABLE_COLUMNS: &str = "SELECT column_name FROM information_schema.columns
    WHERE table_catalog = ? AND table_name = ? ORDER BY ordinal_position";

pub(crate) const COUNT_EXPERIMENTS: &str = "SELECT count(*) FROM experiments";

pub(crate) const COUNT_ACTIVITIES: &str = "SELECT count(*) FROM activity_results";

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        EXPERIMENT_EXISTS,
        AGENTIC_RUN_EXISTS,
        INSERT_AGENTIC_RUN,
        INSERT_CONTRACT_OUTCOME,
        INSERT_FAULT_APPLICATION,
        INSERT_REPLAY_OUTCOME,
        PURGE_EXPERIMENTS,
        ACTIVITIES_EXPORT,
        DENY_EXTERNAL_ACCESS,
        INSERT_AUTOPILOT_DECISION,
        INSERT_AUTOPILOT_EVENT,
        INSERT_CHANGE_EVENT,
        CURRENT_DATABASE,
        TABLE_COLUMNS,
        COUNT_EXPERIMENTS,
        COUNT_ACTIVITIES
    ]
}
