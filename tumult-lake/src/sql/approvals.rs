//! The statements of the approvals of gated runs (`approvals.rs`).

pub(crate) const INSERT_REQUEST: &str =
    "INSERT INTO approval_requests (run_id, tier, pin_hash, env, target, \
    quorum_required, requested_by, requested_at_ns, expires_at_ns) \
    VALUES (?,?,?,?,?,?,?,?,?)";

pub(crate) const INSERT_DECISION: &str =
    "INSERT INTO approval_decisions (run_id, approver, decision, note, decided_at_ns) \
    VALUES (?,?,?,?,?)";

pub(crate) const COUNT_PENDING_RUN: &str = "SELECT COUNT(*) FROM runs WHERE id = ? AND state = ?";

pub(crate) const CONSUME: &str = "UPDATE approval_requests SET consumed_at_ns = ? WHERE run_id = ?";

pub(crate) const BREAK_GLASS: &str =
    "UPDATE approval_requests SET break_glass = TRUE, break_glass_by = ?, \
    break_glass_justification = ? WHERE run_id = ?";

pub(crate) const QUEUE: &str = "SELECT r.id AS run_id, r.state, r.queued_at_ns, r.params_json, \
    g.name AS definition_name, q.*, \
    (SELECT COUNT(*) FROM approval_decisions d \
    WHERE d.run_id = r.id AND d.decision = 'approved') AS approved_count \
    FROM runs r \
    JOIN approval_requests q ON q.run_id = r.id \
    LEFT JOIN run_registry g ON g.id = r.registry_id \
    WHERE r.state = 'pending_approval' \
    ORDER BY q.requested_at_ns";

/// The requester of the approval of the run `run_id`.
pub(crate) fn requester(run_id: &str) -> String {
    format!(
        "SELECT requested_by FROM approval_requests WHERE run_id = '{}'",
        run_id.replace('\'', "''")
    )
}

/// The decision of `approver` on the run `run_id`.
pub(crate) fn decision_of(run_id: &str, approver: &str) -> String {
    format!(
        "SELECT approver FROM approval_decisions WHERE run_id = '{}' AND approver = '{}'",
        run_id.replace('\'', "''"),
        approver.replace('\'', "''")
    )
}

/// The approval request of the run `run_id`.
pub(crate) fn request(run_id: &str) -> String {
    format!(
        "SELECT * FROM approval_requests WHERE run_id = '{}'",
        run_id.replace('\'', "''")
    )
}

/// The decisions on the run `run_id`, oldest first.
pub(crate) fn decisions(run_id: &str) -> String {
    format!(
        "SELECT * FROM approval_decisions WHERE run_id = '{}' ORDER BY decided_at_ns",
        run_id.replace('\'', "''")
    )
}

/// The approval requests with their counts of decisions, newest first.
pub(crate) fn requests(limit: u32) -> String {
    format!(
        "SELECT q.*, g.name AS definition_name, r.state AS run_state, \
         (SELECT COUNT(*) FROM approval_decisions d \
           WHERE d.run_id = q.run_id AND d.decision = 'approved') AS approved_count, \
         (SELECT COUNT(*) FROM approval_decisions d \
           WHERE d.run_id = q.run_id AND d.decision = 'rejected') AS rejected_count \
         FROM approval_requests q \
         LEFT JOIN runs r ON r.id = q.run_id \
         LEFT JOIN run_registry g ON g.id = r.registry_id \
         ORDER BY q.requested_at_ns DESC LIMIT {limit}"
    )
}

/// The runs that still wait for an approval that expired before `now_ns`.
pub(crate) fn expired_pending(now_ns: i64) -> String {
    format!(
        "SELECT r.id AS run_id FROM runs r \
         JOIN approval_requests q ON q.run_id = r.id \
         WHERE r.state = 'pending_approval' AND q.expires_at_ns < {now_ns}"
    )
}

/// The links of the audit chain of the run `run_id`, in order.
pub(crate) fn audit_chain(run_id: &str) -> String {
    format!(
        "SELECT at_ns, event, detail, actor, prev_hash, new_hash FROM run_audit \
         WHERE run_id = '{}' ORDER BY at_ns, rowid",
        run_id.replace('\'', "''")
    )
}

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        INSERT_REQUEST,
        INSERT_DECISION,
        COUNT_PENDING_RUN,
        CONSUME,
        BREAK_GLASS,
        QUEUE,
        requester("run-1"),
        decision_of("run-1", "ada"),
        request("run-1"),
        decisions("run-1"),
        requests(50),
        expired_pending(1_774_980_000_000_000_000),
        audit_chain("run-1")
    ]
}
