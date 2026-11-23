//! The link hashes of the audit trails and of the sign-offs (schema v22).
//!
//! Every audit row of a record or a plan written from this change on has a
//! `link_hash`: the SHA-256 of its own fields and of the `link_hash` of the
//! row before it on the same record or plan. A row that is changed breaks
//! its own link, and a row that is deleted or moved breaks the link of the
//! row after it. Rows written before have no `link_hash`; the chain starts
//! after them.
//!
//! A sign-off row is chained the same way: its `prev_hash` is the
//! `link_hash` of the sign-off before it on the same record or plan.

use duckdb::{params, Connection};
use serde::Serialize;
use serde_json::Value;

use super::hash::{content_hash, now_ns, ulid};
use super::ManualError;
use crate::StoreError;

/// The audit trail of a record or of a plan.
#[derive(Debug, Clone, Copy)]
pub(super) enum Trail {
    Record,
    Plan,
}

impl Trail {
    pub(super) fn table(self) -> &'static str {
        match self {
            Self::Record => "manual_experiment_audit",
            Self::Plan => "manual_plan_audit",
        }
    }

    fn insert_sql(self) -> &'static str {
        match self {
            Self::Record => {
                "INSERT INTO manual_experiment_audit (
                    id, experiment_id, changed_by, changed_at_ns, action, diff,
                    prev_hash, new_hash, link_hash
                 ) VALUES (?,?,?,?,?,CAST(? AS JSON),?,?,?)"
            }
            Self::Plan => {
                "INSERT INTO manual_plan_audit (
                    id, plan_id, changed_by, changed_at_ns, action, diff,
                    prev_hash, new_hash, link_hash
                 ) VALUES (?,?,?,?,?,CAST(? AS JSON),?,?,?)"
            }
        }
    }

    /// The rows of one record or plan, oldest first, with the diff as the
    /// text it is stored as.
    pub(super) fn rows_sql(self) -> &'static str {
        match self {
            Self::Record => {
                "SELECT id, experiment_id AS subject_id, changed_by, changed_at_ns, action, \
                 CAST(diff AS VARCHAR) AS diff, prev_hash, new_hash, link_hash \
                 FROM manual_experiment_audit WHERE experiment_id = ? \
                 ORDER BY changed_at_ns ASC, id ASC"
            }
            Self::Plan => {
                "SELECT id, plan_id AS subject_id, changed_by, changed_at_ns, action, \
                 CAST(diff AS VARCHAR) AS diff, prev_hash, new_hash, link_hash \
                 FROM manual_plan_audit WHERE plan_id = ? \
                 ORDER BY changed_at_ns ASC, id ASC"
            }
        }
    }

    fn last_sql(self) -> &'static str {
        match self {
            Self::Record => {
                "SELECT link_hash, changed_at_ns AS at_ns FROM manual_experiment_audit \
                 WHERE experiment_id = ? ORDER BY changed_at_ns DESC, id DESC LIMIT 1"
            }
            Self::Plan => {
                "SELECT link_hash, changed_at_ns AS at_ns FROM manual_plan_audit \
                 WHERE plan_id = ? ORDER BY changed_at_ns DESC, id DESC LIMIT 1"
            }
        }
    }
}

/// One audit row as its link hash covers it, in this fixed order: never
/// reorder a field. `diff` is the text the row stores.
#[derive(Serialize)]
pub(super) struct AuditLink<'a> {
    pub(super) id: &'a str,
    pub(super) subject_id: &'a str,
    pub(super) changed_by: &'a str,
    pub(super) changed_at_ns: i64,
    pub(super) action: &'a str,
    pub(super) diff: Option<&'a str>,
    pub(super) prev_hash: Option<&'a str>,
    pub(super) new_hash: &'a str,
    /// The `link_hash` of the row before, `None` for the first linked row.
    pub(super) prev_link: Option<&'a str>,
}

/// One sign-off as its link hash covers it, in this fixed order: never
/// reorder a field.
#[derive(Serialize)]
pub(super) struct SignoffLink<'a> {
    pub(super) id: &'a str,
    pub(super) subject_id: &'a str,
    pub(super) subject_kind: &'a str,
    pub(super) role: &'a str,
    pub(super) signed_by: &'a str,
    pub(super) signed_at_ns: i64,
    pub(super) decision: &'a str,
    pub(super) note: Option<&'a str>,
    pub(super) signed_hash: &'a str,
    pub(super) prev_hash: Option<&'a str>,
}

/// The `link_hash` and the time (`at_ns`) of the newest row `sql` reads,
/// so a new row links to it and sorts after it.
fn last_link(
    conn: &Connection,
    sql: &str,
    params: &[&str],
) -> Result<(Option<String>, i64), StoreError> {
    let rows = crate::query_json_rows_bound(conn, sql, params)?;
    let Some(row) = rows.first() else {
        return Ok((None, 0));
    };
    let link = row
        .get("link_hash")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok((link, row.get("at_ns").and_then(Value::as_i64).unwrap_or(0)))
}

/// A time after `last`: now, or one nanosecond after `last` should the
/// clock have gone back, so the rows of one subject keep their order.
fn after(last: i64) -> i64 {
    now_ns().max(last.saturating_add(1))
}

/// Append one audit row to `trail` for `subject_id`, linked to the row
/// before it (no transaction; caller wraps).
#[allow(clippy::too_many_arguments)]
pub(super) fn append_audit(
    conn: &Connection,
    trail: Trail,
    subject_id: &str,
    changed_by: &str,
    action: &str,
    diff: &Value,
    prev_hash: Option<&str>,
    new_hash: &str,
) -> Result<(), ManualError> {
    let (prev_link, last_at) = last_link(conn, trail.last_sql(), &[subject_id])?;
    let id = ulid();
    let changed_at_ns = after(last_at);
    let diff = diff.to_string();
    let link = content_hash(&AuditLink {
        id: &id,
        subject_id,
        changed_by,
        changed_at_ns,
        action,
        diff: Some(&diff),
        prev_hash,
        new_hash,
        prev_link: prev_link.as_deref(),
    });
    conn.execute(
        trail.insert_sql(),
        params![
            id,
            subject_id,
            changed_by,
            changed_at_ns,
            action,
            diff,
            prev_hash,
            new_hash,
            link
        ],
    )?;
    Ok(())
}

/// The sign-offs of one record or plan, oldest first.
pub(super) const SIGNOFFS_SQL: &str = "SELECT * FROM manual_signoffs \
     WHERE subject_kind = ? AND subject_id = ? ORDER BY signed_at_ns ASC, id ASC";

/// The sign-off a subject's next sign-off links to.
const LAST_SIGNOFF_SQL: &str = "SELECT link_hash, signed_at_ns AS at_ns FROM manual_signoffs \
     WHERE subject_kind = ? AND subject_id = ? ORDER BY signed_at_ns DESC, id DESC LIMIT 1";

/// A sign-off about to be written.
pub(super) struct NewSignoff<'a> {
    pub(super) subject_id: &'a str,
    pub(super) subject_kind: &'a str,
    pub(super) role: &'a str,
    pub(super) signed_by: &'a str,
    pub(super) decision: &'a str,
    pub(super) note: Option<&'a str>,
    /// The seal of the subject the sign-off signs.
    pub(super) signed_hash: &'a str,
}

/// Write one sign-off, linked to the one before it on the same subject
/// (no transaction; caller wraps). Returns its id.
pub(super) fn insert_signoff(
    conn: &Connection,
    new: &NewSignoff<'_>,
) -> Result<String, ManualError> {
    let (prev_hash, last_at) =
        last_link(conn, LAST_SIGNOFF_SQL, &[new.subject_kind, new.subject_id])?;
    let id = ulid();
    let signed_at_ns = after(last_at);
    let link = content_hash(&SignoffLink {
        id: &id,
        subject_id: new.subject_id,
        subject_kind: new.subject_kind,
        role: new.role,
        signed_by: new.signed_by,
        signed_at_ns,
        decision: new.decision,
        note: new.note,
        signed_hash: new.signed_hash,
        prev_hash: prev_hash.as_deref(),
    });
    conn.execute(
        "INSERT INTO manual_signoffs (
            id, subject_id, subject_kind, role, signed_by, signed_at_ns, decision, note,
            signed_hash, prev_hash, link_hash
         ) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        params![
            id,
            new.subject_id,
            new.subject_kind,
            new.role,
            new.signed_by,
            signed_at_ns,
            new.decision,
            new.note,
            new.signed_hash,
            prev_hash,
            link
        ],
    )?;
    Ok(id)
}
