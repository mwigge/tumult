//! The statements of the webhooks and their delivery state
//! (`webhooks.rs`).

pub(crate) const INSERT: &str = "INSERT INTO webhooks VALUES (?,?,?,?,?,?,?,?)";

pub(crate) const SET_ENABLED: &str = "UPDATE webhooks SET enabled = ? WHERE id = ?";

pub(crate) const DELETE: &str = "DELETE FROM webhooks WHERE id = ?";

pub(crate) const DELETE_CURSOR: &str = "DELETE FROM webhook_cursors WHERE webhook_id = ?";

pub(crate) const INSERT_CURSOR: &str = "INSERT INTO webhook_cursors VALUES (?,?)";

pub(crate) const INSERT_DEAD_LETTER: &str =
    "INSERT INTO webhook_dead_letters VALUES (?,?,?,?,?,?,?,?,?)";

pub(crate) const LIST: &str = "SELECT * FROM webhooks ORDER BY name";

pub(crate) const LIST_ENABLED: &str = "SELECT * FROM webhooks WHERE enabled ORDER BY name";

/// The delivery cursor of the webhook `webhook_id`.
pub(crate) fn cursor(webhook_id: &str) -> String {
    format!(
        "SELECT last_at_ns FROM webhook_cursors WHERE webhook_id = '{}'",
        webhook_id.replace('\'', "''")
    )
}

/// The statements of this module, for the SQL corpus.
#[cfg(test)]
pub(super) fn sql_corpus() -> Vec<tumult_test_utils::sql_corpus::Statement> {
    tumult_test_utils::sql_statements![
        INSERT,
        SET_ENABLED,
        DELETE,
        DELETE_CURSOR,
        INSERT_CURSOR,
        INSERT_DEAD_LETTER,
        LIST,
        LIST_ENABLED,
        cursor("hook-1")
    ]
}
