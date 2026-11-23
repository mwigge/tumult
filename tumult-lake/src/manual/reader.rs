//! The read side of manual evidence: list and detail queries over the
//! register, its audit trail, its attachments, and the steps, actions and
//! links of a record. Every value is bound, never spliced into the text.

use super::chain::SIGNOFFS_SQL;
use crate::{Reader, StoreError};

/// The steps of one record, in order. The seal reads them too.
pub(super) const STEPS_SQL: &str = "SELECT ordinal, title, actor, started_at_ns, ended_at_ns, \
     result, note FROM manual_exercise_steps WHERE experiment_id = ? ORDER BY ordinal";

/// The actions of one record, in order, with their tracking columns.
pub(super) const ACTIONS_SQL: &str = "SELECT id, ordinal, title, owner, due_at_ns, ticket, \
     status, closed_at_ns, closed_by FROM manual_action_items WHERE experiment_id = ? \
     ORDER BY ordinal";

/// The links of one record, in order.
pub(super) const LINKS_SQL: &str = "SELECT ordinal, kind, linked_id, label \
     FROM manual_experiment_links WHERE experiment_id = ? ORDER BY ordinal";

/// Detail view of one manual experiment: the row, its audit trail, its
/// attachments, its steps, actions and links, and its sign-offs (all as
/// JSON objects straight from the store).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ManualDetail {
    pub experiment: serde_json::Value,
    pub audit: Vec<serde_json::Value>,
    pub attachments: Vec<serde_json::Value>,
    pub steps: Vec<serde_json::Value>,
    pub actions: Vec<serde_json::Value>,
    pub links: Vec<serde_json::Value>,
    /// The sign-offs, oldest first.
    pub signoffs: Vec<serde_json::Value>,
}

impl Reader {
    /// List manual experiments, optionally filtered by lifecycle status.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn manual_experiments(
        &self,
        status: Option<&str>,
    ) -> Result<Vec<serde_json::Value>, StoreError> {
        match status {
            Some(s) => self.query_json_rows_bound(
                "SELECT * FROM manual_experiments WHERE status = ? ORDER BY entered_at_ns DESC",
                &[s],
            ),
            None => {
                self.query_json_rows("SELECT * FROM manual_experiments ORDER BY entered_at_ns DESC")
            }
        }
    }

    /// One manual experiment with its audit trail, attachments, steps,
    /// actions, links and sign-offs.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn manual_experiment_detail(&self, id: &str) -> Result<Option<ManualDetail>, StoreError> {
        let rows =
            self.query_json_rows_bound("SELECT * FROM manual_experiments WHERE id = ?", &[id])?;
        let Some(experiment) = rows.into_iter().next() else {
            return Ok(None);
        };
        let audit = self.query_json_rows_bound(
            "SELECT * FROM manual_experiment_audit WHERE experiment_id = ? \
             ORDER BY changed_at_ns ASC, id ASC",
            &[id],
        )?;
        let attachments = self.query_json_rows_bound(
            "SELECT * FROM evidence_attachments WHERE experiment_id = ? \
             ORDER BY added_at_ns ASC",
            &[id],
        )?;
        Ok(Some(ManualDetail {
            experiment,
            audit,
            attachments,
            steps: self.query_json_rows_bound(STEPS_SQL, &[id])?,
            actions: self.query_json_rows_bound(ACTIONS_SQL, &[id])?,
            links: self.query_json_rows_bound(LINKS_SQL, &[id])?,
            signoffs: self.query_json_rows_bound(SIGNOFFS_SQL, &["record", id])?,
        }))
    }
}
