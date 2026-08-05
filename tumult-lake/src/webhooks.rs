//! Webhook storage (schema v11): the `webhooks` table (admin-managed
//! outbound event sinks) and `webhook_cursors` (one row per webhook — the
//! `run_audit` position the dispatcher has delivered up to). Same additive,
//! index-free rule as the other v5+ tables; the cursor upsert is a
//! delete+insert (the table holds at most one row per webhook).
//!
//! The store keeps the HMAC `secret` (delivery needs it); the service layer
//! must never serialize it out — the API returns it exactly once, at
//! creation, like a one-time password.

use duckdb::params;
use serde_json::Value;

use crate::error::StoreError;
use crate::{Reader, Writer};

/// One `webhooks` row.
#[derive(Debug, Clone, PartialEq)]
pub struct WebhookRow {
    pub id: String,
    pub name: String,
    pub url: String,
    /// HMAC-SHA256 signing key for `X-Tumult-Signature`. Never exposed
    /// after creation.
    pub secret: String,
    /// Audit event names to deliver; empty means every event.
    pub events: Vec<String>,
    pub enabled: bool,
    pub created_by: Option<String>,
    pub created_at_ns: i64,
}

impl Writer {
    /// Insert a webhook row.
    ///
    /// # Errors
    /// Returns an error if the insert fails.
    pub fn create_webhook(&self, w: &WebhookRow) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO webhooks VALUES (?,?,?,?,?,?,?,?)",
            params![
                w.id,
                w.name,
                w.url,
                w.secret,
                serde_json::to_string(&w.events).unwrap_or_default(),
                w.enabled,
                w.created_by,
                w.created_at_ns
            ],
        )?;
        Ok(())
    }

    /// Enable or disable a webhook.
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn set_webhook_enabled(&self, id: &str, enabled: bool) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE webhooks SET enabled = ? WHERE id = ?",
            params![enabled, id],
        )?;
        Ok(())
    }

    /// Delete a webhook (and its delivery cursor) by id.
    ///
    /// # Errors
    /// Returns an error if the delete fails.
    pub fn delete_webhook(&self, id: &str) -> Result<(), StoreError> {
        crate::with_tx(&self.conn, || {
            self.conn
                .execute("DELETE FROM webhooks WHERE id = ?", params![id])?;
            self.conn.execute(
                "DELETE FROM webhook_cursors WHERE webhook_id = ?",
                params![id],
            )?;
            Ok(())
        })
    }

    /// Upsert a webhook's delivery cursor (delete + insert; the table holds
    /// at most one row per webhook).
    ///
    /// # Errors
    /// Returns an error if the write fails.
    pub fn set_webhook_cursor(&self, webhook_id: &str, last_at_ns: i64) -> Result<(), StoreError> {
        crate::with_tx(&self.conn, || {
            self.conn.execute(
                "DELETE FROM webhook_cursors WHERE webhook_id = ?",
                params![webhook_id],
            )?;
            self.conn.execute(
                "INSERT INTO webhook_cursors VALUES (?,?)",
                params![webhook_id, last_at_ns],
            )?;
            Ok(())
        })
    }
}

/// One `webhook_dead_letters` row (schema v13): an audit event the
/// dispatcher gave up delivering after bounded retries.
#[derive(Debug, Clone, PartialEq)]
pub struct WebhookDeadLetter {
    pub webhook_id: String,
    pub run_id: String,
    /// The audit event's original timestamp.
    pub at_ns: i64,
    pub event: String,
    pub detail: Option<String>,
    pub actor: Option<String>,
    /// The last delivery error.
    pub error: String,
    /// Consecutive failed dispatch ticks before giving up.
    pub attempts: u32,
    /// When the dispatcher gave up.
    pub dead_at_ns: i64,
}

impl Writer {
    /// Record a permanently-failed delivery (schema v13). The dispatcher
    /// writes one row per abandoned event *before* advancing the cursor
    /// past it, so delivery loss is never silent.
    ///
    /// # Errors
    /// Returns an error if the insert fails.
    pub fn insert_webhook_dead_letter(&self, d: &WebhookDeadLetter) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO webhook_dead_letters VALUES (?,?,?,?,?,?,?,?,?)",
            params![
                d.webhook_id,
                d.run_id,
                d.at_ns,
                d.event,
                d.detail,
                d.actor,
                d.error,
                d.attempts,
                d.dead_at_ns
            ],
        )?;
        Ok(())
    }
}

impl Reader {
    /// List all webhooks, ordered by name. Includes `secret` — the service
    /// layer decides what to expose; the API must never serialize it out.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn list_webhooks(&self) -> Result<Vec<WebhookRow>, StoreError> {
        let rows = self.query_json_rows("SELECT * FROM webhooks ORDER BY name")?;
        Ok(rows.iter().map(row_to_webhook).collect())
    }

    /// List enabled webhooks (the dispatcher's selection).
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn enabled_webhooks(&self) -> Result<Vec<WebhookRow>, StoreError> {
        let rows = self.query_json_rows("SELECT * FROM webhooks WHERE enabled ORDER BY name")?;
        Ok(rows.iter().map(row_to_webhook).collect())
    }

    /// A webhook's delivery cursor, or `None` when nothing was delivered yet.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn webhook_cursor(&self, webhook_id: &str) -> Result<Option<i64>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT last_at_ns FROM webhook_cursors WHERE webhook_id = '{}'",
            webhook_id.replace('\'', "''")
        ))?;
        Ok(rows.first().and_then(|r| r["last_at_ns"].as_i64()))
    }
}

fn row_to_webhook(v: &Value) -> WebhookRow {
    let s = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    WebhookRow {
        id: s("id"),
        name: s("name"),
        url: s("url"),
        secret: s("secret"),
        events: serde_json::from_str(v["events"].as_str().unwrap_or("[]")).unwrap_or_default(),
        enabled: v["enabled"].as_bool().unwrap_or(false),
        created_by: v["created_by"].as_str().map(str::to_string),
        created_at_ns: v["created_at_ns"].as_i64().unwrap_or(0),
    }
}
