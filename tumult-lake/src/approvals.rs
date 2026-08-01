//! Approval storage (schema v7): `approval_requests` pins one run to exactly
//! one canonical content hash with a TTL and a quorum; `approval_decisions`
//! holds one row per approver decision. T0 runs never gate and appear in
//! neither table.
//!
//! The same schema version extends `run_audit` with a per-run hash chain
//! (`prev_hash` / `new_hash`), making the trail tamper-evident like
//! `manual_experiment_audit`: every event hashes its own content plus the
//! previous link's hash, so rewriting or deleting an event breaks every
//! later link. Rows written before v7 carry NULL hashes and are treated as
//! legacy by [`Reader::verify_run_audit_chain`].
//!
//! Tier *classification* lives in `tumult_ingest::approvals` (it needs the
//! parsed experiment); this module stores the outcome.

use std::collections::BTreeMap;

use duckdb::params;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::StoreError;
use crate::{Reader, Writer};

/// `approval_decisions.decision` values.
pub mod decision {
    pub const APPROVED: &str = "approved";
    pub const REJECTED: &str = "rejected";
}

/// The exact content an approval approves: the definition source, the
/// template params, the target environment, and the optional target
/// selector (ADR-013).
///
/// The pin covers the resolution *inputs*, not the resolved artifact:
/// `prepare_run` is a pure function of (`definition_toon`, `params`), so
/// pinning the inputs pins whatever the worker later resolves — while
/// avoiding the resolved `Experiment`'s nondeterministic serialization
/// (its `HashMap` fields iterate in random order). Any edit to any input
/// after approval changes the pin and the dispatch re-verification refuses.
#[derive(Serialize)]
pub struct CanonicalPin<'a> {
    pub definition_toon: &'a str,
    pub params: &'a BTreeMap<String, String>,
    pub env: &'a str,
    pub target: Option<&'a str>,
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Lowercase SHA-256 hex of the canonical pin. `serde_json` serializes
/// struct fields in declaration order and [`BTreeMap`] keys sorted, so the
/// hash is deterministic across processes and map insertion orders.
#[must_use]
pub fn approval_pin(pin: &CanonicalPin<'_>) -> String {
    let json = serde_json::to_string(pin).unwrap_or_default();
    sha256_hex(json.as_bytes())
}

/// One link of the `run_audit` hash chain (schema v7). Fixed field order;
/// `None`s serialize as JSON nulls, so legacy-free rows hash deterministically.
#[derive(Serialize)]
struct AuditLink<'a> {
    run_id: &'a str,
    at_ns: i64,
    event: &'a str,
    detail: Option<&'a str>,
    actor: Option<&'a str>,
    prev_hash: Option<&'a str>,
}

/// The `new_hash` of one `run_audit` row: SHA-256 over the event content
/// and the previous link's hash (`None` for the first link of a run).
#[must_use]
pub fn audit_chain_hash(
    run_id: &str,
    at_ns: i64,
    event: &str,
    detail: Option<&str>,
    actor: Option<&str>,
    prev_hash: Option<&str>,
) -> String {
    let link = AuditLink {
        run_id,
        at_ns,
        event,
        detail,
        actor,
        prev_hash,
    };
    let json = serde_json::to_string(&link).unwrap_or_default();
    sha256_hex(json.as_bytes())
}

/// An approval request at insert time (`approval_requests`).
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub run_id: String,
    /// `T1` | `T2` | `T3` (T0 never gates).
    pub tier: String,
    pub pin_hash: String,
    pub env: String,
    pub target: Option<String>,
    pub quorum_required: i64,
    pub requested_by: String,
    pub requested_at_ns: i64,
    pub expires_at_ns: i64,
}

/// One approver's decision (`approval_decisions`).
#[derive(Debug, Clone)]
pub struct ApprovalDecision {
    pub run_id: String,
    pub approver: String,
    /// [`decision::APPROVED`] | [`decision::REJECTED`].
    pub decision: String,
    pub note: Option<String>,
    pub decided_at_ns: i64,
}

impl Writer {
    /// Persist a gated run: the run row in `pending_approval` state, its
    /// approval request, and the `requested` audit event (carrying tier,
    /// quorum, TTL and pin in `detail`), in one transaction. No work item is
    /// queued — dispatch happens via the approval flow (T10).
    ///
    /// # Errors
    /// Returns an error if any row fails to insert.
    pub fn insert_gated_run(
        &self,
        run: &crate::NewRun,
        req: &ApprovalRequest,
        detail: Option<&str>,
    ) -> Result<(), StoreError> {
        crate::with_tx(&self.conn, || {
            self.conn.execute(
                "INSERT INTO runs (id, registry_id, state, params_json, queued_at_ns) \
                 VALUES (?,?,?,CAST(? AS JSON),?)",
                params![
                    run.id,
                    run.registry_id,
                    crate::run_state::PENDING_APPROVAL,
                    run.params_json,
                    run.queued_at_ns
                ],
            )?;
            self.insert_approval_request(req)?;
            self.insert_run_audit(&run.id, "requested", detail, run.actor.as_deref())
        })
    }

    /// Record an approval request for a gated run.
    ///
    /// # Errors
    /// Returns an error if the row fails to insert.
    pub fn insert_approval_request(&self, req: &ApprovalRequest) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO approval_requests (run_id, tier, pin_hash, env, target, \
             quorum_required, requested_by, requested_at_ns, expires_at_ns) \
             VALUES (?,?,?,?,?,?,?,?,?)",
            params![
                req.run_id,
                req.tier,
                req.pin_hash,
                req.env,
                req.target,
                req.quorum_required,
                req.requested_by,
                req.requested_at_ns,
                req.expires_at_ns
            ],
        )?;
        Ok(())
    }

    /// Record one approver's decision. Enforces the code-level invariants the
    /// index-free schema cannot: one decision per (run, approver), and
    /// segregation of duties — the approver never equals the requester.
    ///
    /// # Errors
    /// Returns [`StoreError::Internal`] on a duplicate decision or a
    /// self-approval attempt, and on missing approval request.
    pub fn insert_approval_decision(&self, dec: &ApprovalDecision) -> Result<(), StoreError> {
        let request = crate::query_json_rows(
            &self.conn,
            &format!(
                "SELECT requested_by FROM approval_requests WHERE run_id = '{}'",
                dec.run_id.replace('\'', "''")
            ),
        )?;
        let Some(request) = request.first() else {
            return Err(StoreError::Internal(format!(
                "no approval request for run {}",
                dec.run_id
            )));
        };
        if request["requested_by"].as_str() == Some(dec.approver.as_str()) {
            return Err(StoreError::Internal(format!(
                "approver {} is the requester — self-approval is forbidden",
                dec.approver
            )));
        }
        let existing = crate::query_json_rows(
            &self.conn,
            &format!(
                "SELECT approver FROM approval_decisions WHERE run_id = '{}' AND approver = '{}'",
                dec.run_id.replace('\'', "''"),
                dec.approver.replace('\'', "''")
            ),
        )?;
        if !existing.is_empty() {
            return Err(StoreError::Internal(format!(
                "approver {} already decided on run {}",
                dec.approver, dec.run_id
            )));
        }
        self.conn.execute(
            "INSERT INTO approval_decisions (run_id, approver, decision, note, decided_at_ns) \
             VALUES (?,?,?,?,?)",
            params![
                dec.run_id,
                dec.approver,
                dec.decision,
                dec.note,
                dec.decided_at_ns
            ],
        )?;
        Ok(())
    }

    /// Stamp the approval consumed — single-use: one dispatch consumes one
    /// approval, a second run needs a fresh approval (ADR-013).
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn consume_approval(&self, run_id: &str, consumed_at_ns: i64) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE approval_requests SET consumed_at_ns = ? WHERE run_id = ?",
            params![consumed_at_ns, run_id],
        )?;
        Ok(())
    }

    /// Mark the request overridden by break-glass (admin override with
    /// mandatory justification, ADR-013).
    ///
    /// # Errors
    /// Returns an error if the update fails.
    pub fn mark_break_glass(
        &self,
        run_id: &str,
        by: &str,
        justification: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE approval_requests SET break_glass = TRUE, break_glass_by = ?, \
             break_glass_justification = ? WHERE run_id = ?",
            params![by, justification, run_id],
        )?;
        Ok(())
    }
}

impl Reader {
    /// The approval request for a run, or `None` (T0 runs have none).
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn approval_request(&self, run_id: &str) -> Result<Option<serde_json::Value>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT * FROM approval_requests WHERE run_id = '{}'",
            run_id.replace('\'', "''")
        ))?;
        Ok(rows.into_iter().next())
    }

    /// All decisions on a run, oldest first.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn approval_decisions(&self, run_id: &str) -> Result<Vec<serde_json::Value>, StoreError> {
        self.query_json_rows(&format!(
            "SELECT * FROM approval_decisions WHERE run_id = '{}' ORDER BY decided_at_ns",
            run_id.replace('\'', "''")
        ))
    }

    /// The approval queue: `pending_approval` runs joined with their request,
    /// definition name, and the count of approvals collected so far.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn approvals_queue(&self) -> Result<Vec<serde_json::Value>, StoreError> {
        self.query_json_rows(
            "SELECT r.id AS run_id, r.state, r.queued_at_ns, r.params_json, \
             g.name AS definition_name, q.*, \
             (SELECT COUNT(*) FROM approval_decisions d \
               WHERE d.run_id = r.id AND d.decision = 'approved') AS approved_count \
             FROM runs r \
             JOIN approval_requests q ON q.run_id = r.id \
             LEFT JOIN run_registry g ON g.id = r.registry_id \
             WHERE r.state = 'pending_approval' \
             ORDER BY q.requested_at_ns",
        )
    }

    /// Every approval request with its run's definition name and decision
    /// count, newest first — the R2 evidence pack's approval-chain source.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn approvals_list(&self, limit: u32) -> Result<Vec<serde_json::Value>, StoreError> {
        self.query_json_rows(&format!(
            "SELECT q.*, g.name AS definition_name, r.state AS run_state, \
             (SELECT COUNT(*) FROM approval_decisions d \
               WHERE d.run_id = q.run_id AND d.decision = 'approved') AS approved_count, \
             (SELECT COUNT(*) FROM approval_decisions d \
               WHERE d.run_id = q.run_id AND d.decision = 'rejected') AS rejected_count \
             FROM approval_requests q \
             LEFT JOIN runs r ON r.id = q.run_id \
             LEFT JOIN run_registry g ON g.id = r.registry_id \
             ORDER BY q.requested_at_ns DESC LIMIT {limit}"
        ))
    }

    /// Gated runs whose approval TTL has lapsed (`now_ns` > `expires_at_ns`)
    /// and that are still waiting — the expiry sweeper's input.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn expired_pending_approvals(&self, now_ns: i64) -> Result<Vec<String>, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT r.id AS run_id FROM runs r \
             JOIN approval_requests q ON q.run_id = r.id \
             WHERE r.state = 'pending_approval' AND q.expires_at_ns < {now_ns}"
        ))?;
        Ok(rows
            .iter()
            .filter_map(|r| r["run_id"].as_str().map(str::to_string))
            .collect())
    }

    /// Re-verify a run's audit hash chain (schema v7). Legacy rows (NULL
    /// `new_hash`, written before v7) are skipped; the first chained row may
    /// legitimately have a NULL `prev_hash`. Returns `false` if any link's
    /// stored hash does not recompute, or a link's `prev_hash` does not equal
    /// the previous link's `new_hash` — i.e. the trail was tampered with.
    ///
    /// # Errors
    /// Returns an error if the query fails.
    pub fn verify_run_audit_chain(&self, run_id: &str) -> Result<bool, StoreError> {
        let rows = self.query_json_rows(&format!(
            "SELECT at_ns, event, detail, actor, prev_hash, new_hash FROM run_audit \
             WHERE run_id = '{}' ORDER BY at_ns, rowid",
            run_id.replace('\'', "''")
        ))?;
        let mut expected_prev: Option<String> = None;
        for row in rows {
            let Some(new_hash) = row["new_hash"].as_str() else {
                continue; // legacy pre-v7 row
            };
            let prev_hash = row["prev_hash"].as_str();
            if prev_hash != expected_prev.as_deref() {
                return Ok(false);
            }
            let recomputed = audit_chain_hash(
                run_id,
                row["at_ns"].as_i64().unwrap_or(0),
                row["event"].as_str().unwrap_or_default(),
                row["detail"].as_str(),
                row["actor"].as_str(),
                prev_hash,
            );
            if recomputed != new_hash {
                return Ok(false);
            }
            expected_prev = Some(new_hash.to_string());
        }
        Ok(true)
    }
}
