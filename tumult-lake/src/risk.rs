//! Risk suppression storage (schema v17): `risk_suppressions`, the
//! append-only record of passive-scan findings an operator accepted for a
//! while. Rows are never updated or deleted; a later row for the same
//! `risk_id` supersedes the earlier one, and each row links to the previous
//! one for that risk id through the same hash chain as `run_audit`, so an
//! auditor can verify that nothing was silenced off the record.

use duckdb::params;
use serde_json::Value;

use crate::error::StoreError;
use crate::{Reader, Writer};

/// One `risk_suppressions` row.
#[derive(Debug, Clone, PartialEq)]
pub struct RiskSuppressionRow {
    pub id: String,
    pub risk_id: String,
    pub reason: String,
    pub actor: String,
    pub at_ns: i64,
    pub expires_at_ns: Option<i64>,
    pub prev_hash: Option<String>,
    pub new_hash: String,
}

impl Writer {
    /// Append a suppression, chaining it onto the latest row for the same
    /// risk id. Returns the stored row (with its hash link filled in).
    ///
    /// # Errors
    /// Returns an error if the lookup or the insert fails.
    pub fn insert_risk_suppression(
        &self,
        id: &str,
        risk_id: &str,
        reason: &str,
        actor: &str,
        at_ns: i64,
        expires_at_ns: Option<i64>,
    ) -> Result<RiskSuppressionRow, StoreError> {
        let prev_hash: Option<String> = match self.conn.query_row(
            "SELECT new_hash FROM risk_suppressions WHERE risk_id = ? \
             ORDER BY at_ns DESC, rowid DESC LIMIT 1",
            params![risk_id],
            |row| row.get::<_, Option<String>>(0),
        ) {
            Ok(hash) => hash,
            Err(duckdb::Error::QueryReturnedNoRows) => None,
            Err(error) => return Err(error.into()),
        };
        let detail = serde_json::json!({
            "id": id,
            "reason": reason,
            "expires_at_ns": expires_at_ns,
        })
        .to_string();
        let new_hash = crate::approvals::audit_chain_hash(
            risk_id,
            at_ns,
            "risk.suppressed",
            Some(&detail),
            Some(actor),
            prev_hash.as_deref(),
        );
        self.conn.execute(
            "INSERT INTO risk_suppressions \
             (id, risk_id, reason, actor, at_ns, expires_at_ns, prev_hash, new_hash) \
             VALUES (?,?,?,?,?,?,?,?)",
            params![
                id,
                risk_id,
                reason,
                actor,
                at_ns,
                expires_at_ns,
                prev_hash,
                new_hash
            ],
        )?;
        Ok(RiskSuppressionRow {
            id: id.to_string(),
            risk_id: risk_id.to_string(),
            reason: reason.to_string(),
            actor: actor.to_string(),
            at_ns,
            expires_at_ns,
            prev_hash,
            new_hash,
        })
    }
}

impl Reader {
    /// Every suppression row, oldest first. The scanner picks the latest
    /// active row per risk id itself.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn list_risk_suppressions(&self) -> Result<Vec<RiskSuppressionRow>, StoreError> {
        let rows =
            self.query_json_rows("SELECT * FROM risk_suppressions ORDER BY at_ns ASC, rowid ASC")?;
        Ok(rows.iter().map(row_to_suppression).collect())
    }
}

fn str_field(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn row_to_suppression(row: &Value) -> RiskSuppressionRow {
    RiskSuppressionRow {
        id: str_field(row, "id"),
        risk_id: str_field(row, "risk_id"),
        reason: str_field(row, "reason"),
        actor: str_field(row, "actor"),
        at_ns: row.get("at_ns").and_then(Value::as_i64).unwrap_or_default(),
        expires_at_ns: row.get("expires_at_ns").and_then(Value::as_i64),
        prev_hash: row
            .get("prev_hash")
            .and_then(Value::as_str)
            .map(str::to_owned),
        new_hash: str_field(row, "new_hash"),
    }
}

#[cfg(test)]
mod tests {
    use crate::Store;

    #[test]
    fn suppressions_append_and_chain_per_risk_id() {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let first = writer
            .insert_risk_suppression("s1", "r_abc", "accepted until Q1", "op", 100, Some(200))
            .unwrap();
        let second = writer
            .insert_risk_suppression("s2", "r_abc", "extended", "op", 150, None)
            .unwrap();
        let other = writer
            .insert_risk_suppression("s3", "r_xyz", "other finding", "op", 120, None)
            .unwrap();
        assert_eq!(first.prev_hash, None);
        assert_eq!(second.prev_hash.as_deref(), Some(first.new_hash.as_str()));
        assert_eq!(other.prev_hash, None, "chains are per risk id");
        assert_ne!(first.new_hash, second.new_hash);

        let reader = store.read_only().unwrap();
        let rows = reader.list_risk_suppressions().unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], first);
        assert_eq!(rows[1], other, "oldest first");
        assert_eq!(rows[2], second);
        assert_eq!(rows[2].expires_at_ns, None);
    }
}
