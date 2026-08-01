//! The read side of manual evidence: list and detail queries over the
//! register, its audit trail, and its attachments.

use crate::{Reader, StoreError};

/// Detail view of one manual experiment: the row, its audit trail and its
/// attachments (all as JSON objects straight from the store).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ManualDetail {
    pub experiment: serde_json::Value,
    pub audit: Vec<serde_json::Value>,
    pub attachments: Vec<serde_json::Value>,
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
        let sql = match status {
            Some(s) => format!(
                "SELECT * FROM manual_experiments WHERE status = '{}' \
                 ORDER BY entered_at_ns DESC",
                s.replace('\'', "''")
            ),
            None => "SELECT * FROM manual_experiments ORDER BY entered_at_ns DESC".to_string(),
        };
        self.query_json_rows(&sql)
    }

    /// One manual experiment with its audit trail and attachments.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn manual_experiment_detail(&self, id: &str) -> Result<Option<ManualDetail>, StoreError> {
        let id = id.replace('\'', "''");
        let rows = self.query_json_rows(&format!(
            "SELECT * FROM manual_experiments WHERE id = '{id}'"
        ))?;
        let Some(experiment) = rows.into_iter().next() else {
            return Ok(None);
        };
        let audit = self.query_json_rows(&format!(
            "SELECT * FROM manual_experiment_audit WHERE experiment_id = '{id}' \
             ORDER BY changed_at_ns ASC, id ASC"
        ))?;
        let attachments = self.query_json_rows(&format!(
            "SELECT * FROM evidence_attachments WHERE experiment_id = '{id}' \
             ORDER BY added_at_ns ASC"
        ))?;
        Ok(Some(ManualDetail {
            experiment,
            audit,
            attachments,
        }))
    }
}
