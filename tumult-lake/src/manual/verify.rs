//! The audit verify of a record or a plan: everything the lake holds about
//! it is checked again, so a direct edit of a row or a deleted row shows.
//!
//! For a record: the stored seal is recomputed in the form of its
//! `hash_version`; the audit rows must chain (each `prev_hash` is the
//! `new_hash` of the row before, and each `link_hash` covers the row and
//! the one before); the last `new_hash` must be the stored seal; the
//! reviewer must be the one of the last verify or reject row; and the
//! sign-offs must chain, each signing a seal the audit trail holds and
//! each named by its audit row. Each attachment must be named by its attach
//! row and say what that row says, each attach row must name an attachment
//! that is there, and each stored file must be there with the bytes of its
//! hash. A plan is checked the same way, without the reviewer and the
//! attachments. Audit rows with no
//! `link_hash` at the start of a trail were written before the link hash
//! existed: they are counted as legacy rows, not as problems.

use std::collections::BTreeSet;
use std::path::Path;

use duckdb::Connection;
use serde::Serialize;
use serde_json::Value;

use super::chain::{AuditLink, SignoffLink, Trail, SIGNOFFS_SQL};
use super::files::{attachments_dir, stored_file};
use super::hash::{content_hash, fetch_plan_row, fetch_row, plan_seal_of, seal_of};
use crate::sql;
use crate::{Reader, StoreError};

/// One thing the verify found wrong, and the row it is in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditProblem {
    /// The table of the row.
    pub table: String,
    /// The id of the row.
    pub row: String,
    /// The column at fault, when the check can tell which one.
    pub field: Option<String>,
    pub problem: String,
}

/// The verdict of the audit verify of one record or plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditVerdict {
    /// No problem found.
    pub ok: bool,
    /// The form the seal of a record is in; `None` (and left out) for a
    /// plan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hash_version: Option<i64>,
    /// The audit rows at the start of the trail that were written before
    /// the link hash.
    pub legacy_rows: usize,
    pub problems: Vec<AuditProblem>,
}

/// The problems of one record or plan, as they are found.
struct Problems {
    found: Vec<AuditProblem>,
}

impl Problems {
    fn add(&mut self, table: &str, row: &str, field: Option<&str>, problem: String) {
        self.found.push(AuditProblem {
            table: table.into(),
            row: row.into(),
            field: field.map(str::to_owned),
            problem,
        });
    }
}

fn text<'a>(row: &'a Value, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

/// Check the audit rows of `subject_id` in one trail against its stored
/// `seal`; returns the number of legacy rows.
fn check_trail(
    table: &str,
    subject_id: &str,
    rows: &[Value],
    seal: &str,
    problems: &mut Problems,
) -> usize {
    let mut seal_before: Option<&str> = None;
    let mut link_before: Option<&str> = None;
    let mut legacy = 0;
    for row in rows {
        let id = text(row, "id").unwrap_or_default();
        let prev_hash = text(row, "prev_hash");
        let new_hash = text(row, "new_hash");
        if prev_hash != seal_before {
            problems.add(
                table,
                id,
                Some("prev_hash"),
                "prev_hash is not the new_hash of the row before it: a row was deleted, \
                 moved or changed"
                    .into(),
            );
        }
        seal_before = new_hash;
        match (text(row, "link_hash"), link_before) {
            (None, None) => legacy += 1,
            (None, Some(_)) => problems.add(
                table,
                id,
                Some("link_hash"),
                "the row has no link_hash, but the rows before it do".into(),
            ),
            (Some(link), _) => {
                let want = content_hash(&AuditLink {
                    id,
                    subject_id: text(row, "subject_id").unwrap_or_default(),
                    changed_by: text(row, "changed_by").unwrap_or_default(),
                    changed_at_ns: row
                        .get("changed_at_ns")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                    action: text(row, "action").unwrap_or_default(),
                    diff: text(row, "diff"),
                    prev_hash,
                    new_hash: new_hash.unwrap_or_default(),
                    prev_link: link_before,
                });
                if want != link {
                    problems.add(
                        table,
                        id,
                        None,
                        "the row does not match its link_hash: it was changed, or the row \
                         before it was deleted or moved"
                            .into(),
                    );
                }
                link_before = Some(link);
            }
        }
    }
    match rows.last() {
        None => problems.add(table, subject_id, None, "there is no audit row".into()),
        Some(last) if text(last, "new_hash") != Some(seal) => problems.add(
            table,
            text(last, "id").unwrap_or_default(),
            Some("new_hash"),
            "the new_hash of the last row is not the stored seal: a later row was \
             deleted, or the seal was changed"
                .into(),
        ),
        Some(_) => {}
    }
    legacy
}

/// Check the sign-offs of one subject: they chain, and each signs one of
/// `seals`, the seals its audit trail holds.
fn check_signoffs(rows: &[Value], seals: &BTreeSet<&str>, problems: &mut Problems) {
    const TABLE: &str = "manual_signoffs";
    let mut link_before: Option<&str> = None;
    for row in rows {
        let id = text(row, "id").unwrap_or_default();
        let prev_hash = text(row, "prev_hash");
        if prev_hash != link_before {
            problems.add(
                TABLE,
                id,
                Some("prev_hash"),
                "prev_hash is not the link_hash of the sign-off before it: a sign-off was \
                 deleted or moved"
                    .into(),
            );
        }
        let signed_hash = text(row, "signed_hash").unwrap_or_default();
        let want = content_hash(&SignoffLink {
            id,
            subject_id: text(row, "subject_id").unwrap_or_default(),
            subject_kind: text(row, "subject_kind").unwrap_or_default(),
            role: text(row, "role").unwrap_or_default(),
            signed_by: text(row, "signed_by").unwrap_or_default(),
            signed_at_ns: row.get("signed_at_ns").and_then(Value::as_i64).unwrap_or(0),
            decision: text(row, "decision").unwrap_or_default(),
            note: text(row, "note"),
            signed_hash,
            prev_hash,
        });
        let link = text(row, "link_hash");
        if link != Some(want.as_str()) {
            problems.add(
                TABLE,
                id,
                None,
                "the sign-off does not match its link_hash: it was changed".into(),
            );
        }
        if !seals.contains(signed_hash) {
            problems.add(
                TABLE,
                id,
                Some("signed_hash"),
                "signed_hash is not a seal of this record in its audit trail".into(),
            );
        }
        link_before = link;
    }
}

/// Each sign-off has the audit row that names it by `signoff_id`, and each
/// sign-off an audit row names is there. The audit rows are linked, so a
/// deleted sign-off, or a deleted last audit row of a sign-off, shows here.
fn check_signoff_rows(audit: &[Value], signoffs: &[Value], problems: &mut Problems) {
    let named: Vec<(&str, String)> = audit
        .iter()
        .filter_map(|row| {
            let diff: Value = serde_json::from_str(text(row, "diff")?).ok()?;
            let signoff = diff.get("signoff_id")?.as_str()?.to_owned();
            Some((text(row, "id").unwrap_or_default(), signoff))
        })
        .collect();
    let ids: BTreeSet<&str> = signoffs.iter().filter_map(|s| text(s, "id")).collect();
    for (row, signoff) in &named {
        if !ids.contains(signoff.as_str()) {
            problems.add(
                Trail::Record.table(),
                row,
                Some("diff"),
                format!("the row names sign-off '{signoff}', which is not in manual_signoffs"),
            );
        }
    }
    for id in ids {
        if !named.iter().any(|(_, signoff)| signoff == id) {
            problems.add(
                "manual_signoffs",
                id,
                None,
                "no audit row names this sign-off: its audit row was deleted".into(),
            );
        }
    }
}

/// The columns of an attachment that its attach audit row names.
const ATTACHED: [&str; 6] = [
    "kind",
    "uri",
    "label",
    "file_hash",
    "media_type",
    "size_bytes",
];

/// The table of the attachments.
const ATTACHMENTS: &str = "evidence_attachments";

/// Each attachment has the attach audit row that names it by
/// `attachment_id` and holds what that row says, and each attachment an
/// attach row names is there. An attach leaves the seal as it was, so a
/// deleted last attach row shows only here. The file of each file
/// attachment is checked under `files`.
fn check_attachments(
    files: &Path,
    audit: &[Value],
    attachments: &[Value],
    problems: &mut Problems,
) {
    let named: Vec<(&str, Value)> = audit
        .iter()
        .filter(|row| text(row, "action") == Some("attach"))
        .filter_map(|row| {
            let diff = serde_json::from_str(text(row, "diff")?).ok()?;
            Some((text(row, "id").unwrap_or_default(), diff))
        })
        .collect();
    let names =
        |diff: &Value, id: &str| diff.get("attachment_id").and_then(Value::as_str) == Some(id);
    for (row, diff) in &named {
        let id = diff
            .get("attachment_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !attachments.iter().any(|a| text(a, "id") == Some(id)) {
            problems.add(
                Trail::Record.table(),
                row,
                Some("diff"),
                format!("the row names attachment '{id}', which is not in {ATTACHMENTS}"),
            );
        }
    }
    for attachment in attachments {
        let id = text(attachment, "id").unwrap_or_default();
        let Some((_, diff)) = named.iter().find(|(_, diff)| names(diff, id)) else {
            problems.add(
                ATTACHMENTS,
                id,
                None,
                "no audit row names this attachment: its attach row was deleted".into(),
            );
            continue;
        };
        for field in ATTACHED {
            if diff
                .get(field)
                .is_some_and(|want| attachment.get(field) != Some(want))
            {
                problems.add(
                    ATTACHMENTS,
                    id,
                    Some(field),
                    format!("{field} is not what the attach row of the audit trail names"),
                );
            }
        }
        if text(attachment, "kind") == Some("file") {
            check_file(files, id, text(attachment, "file_hash"), problems);
        }
    }
}

/// The file of attachment `id` is stored under `files` and hashes to
/// `hash`.
fn check_file(files: &Path, id: &str, hash: Option<&str>, problems: &mut Problems) {
    let field = Some("file_hash");
    let Some(path) = hash.and_then(|hash| stored_file(files, hash)) else {
        problems.add(ATTACHMENTS, id, field, "file_hash is not a SHA-256".into());
        return;
    };
    if !path.is_file() {
        problems.add(
            ATTACHMENTS,
            id,
            field,
            "the stored file is missing: it was deleted or moved".into(),
        );
        return;
    }
    match crate::file_integrity::checksum(&path) {
        Ok(found) if Some(found.as_str()) == hash => {}
        Ok(_) => problems.add(
            ATTACHMENTS,
            id,
            field,
            "the stored file does not hash to file_hash: it was changed".into(),
        ),
        Err(e) => problems.add(
            ATTACHMENTS,
            id,
            field,
            format!("the stored file cannot be read: {e}"),
        ),
    }
}

/// Every seal the audit rows hold, before and after each write.
fn seals_of(rows: &[Value]) -> BTreeSet<&str> {
    rows.iter()
        .flat_map(|r| [text(r, "prev_hash"), text(r, "new_hash")])
        .flatten()
        .collect()
}

/// The reviewer of a verified or rejected record is the one of its last
/// verify or reject row.
fn check_reviewer(id: &str, row: &Value, audit: &[Value], problems: &mut Problems) {
    let action = match text(row, "status") {
        Some("verified") => "verify",
        Some("rejected") => "reject",
        _ => return,
    };
    let by = audit
        .iter()
        .rev()
        .find(|a| text(a, "action") == Some(action))
        .and_then(|a| text(a, "changed_by"));
    let reviewed_by = text(row, "reviewed_by");
    if reviewed_by != by {
        problems.add(
            "manual_experiments",
            id,
            Some("reviewed_by"),
            format!(
                "reviewed_by is {}, but the {action} row of the audit trail is by {}",
                reviewed_by.map_or_else(|| "empty".into(), |r| format!("'{r}'")),
                by.map_or_else(|| "nobody".into(), |b| format!("'{b}'")),
            ),
        );
    }
}

/// Compare the seal recomputed in the form of `version` with the stored one.
fn check_record_seal(
    conn: &Connection,
    id: &str,
    row: &Value,
    version: i64,
    problems: &mut Problems,
) -> Result<(), StoreError> {
    const TABLE: &str = "manual_experiments";
    if !(1..=super::hash::HASH_VERSION).contains(&version) {
        problems.add(
            TABLE,
            id,
            Some("hash_version"),
            format!("hash_version {version} is not a form of the seal"),
        );
        return Ok(());
    }
    let seal = match seal_of(conn, id, row, version) {
        Ok(seal) => seal,
        // A column that no longer reads as its type was changed.
        Err(StoreError::Json(e)) => {
            problems.add(TABLE, id, None, format!("the seal cannot be computed: {e}"));
            return Ok(());
        }
        Err(e) => return Err(e),
    };
    if text(row, "content_hash") != Some(seal.as_str()) {
        let covers = if version == 1 {
            "a column of the record was changed after it was sealed (a version 1 seal does \
             not cover the reviewer, the steps, the actions or the links)"
        } else {
            "a column of the record, a step, an action or a link was changed after it was \
             sealed"
        };
        problems.add(
            TABLE,
            id,
            None,
            format!("the stored seal is not the seal of the record: {covers}"),
        );
    }
    Ok(())
}

fn verdict(problems: Problems, hash_version: Option<i64>, legacy_rows: usize) -> AuditVerdict {
    AuditVerdict {
        ok: problems.found.is_empty(),
        hash_version,
        legacy_rows,
        problems: problems.found,
    }
}

/// The audit verify of record `id`, whose stored files are under
/// `files`; `None` when there is no such record.
pub(crate) fn verify_record(
    conn: &Connection,
    id: &str,
    files: &Path,
) -> Result<Option<AuditVerdict>, StoreError> {
    let Some(row) = fetch_row(conn, id)? else {
        return Ok(None);
    };
    let mut problems = Problems { found: Vec::new() };
    let version = row.get("hash_version").and_then(Value::as_i64).unwrap_or(1);
    check_record_seal(conn, id, &row, version, &mut problems)?;
    let audit = crate::query_json_rows_bound(conn, Trail::Record.rows_sql(), &[id])?;
    let seal = text(&row, "content_hash").unwrap_or_default();
    let legacy = check_trail(Trail::Record.table(), id, &audit, seal, &mut problems);
    check_reviewer(id, &row, &audit, &mut problems);
    let signoffs = crate::query_json_rows_bound(conn, SIGNOFFS_SQL, &["record", id])?;
    check_signoffs(&signoffs, &seals_of(&audit), &mut problems);
    check_signoff_rows(&audit, &signoffs, &mut problems);
    let attachments = crate::query_json_rows_bound(conn, sql::manual::ATTACHMENTS_IN_ORDER, &[id])?;
    check_attachments(files, &audit, &attachments, &mut problems);
    Ok(Some(verdict(problems, Some(version), legacy)))
}

/// The audit verify of plan `id`; `None` when there is no such plan.
pub(crate) fn verify_plan(conn: &Connection, id: &str) -> Result<Option<AuditVerdict>, StoreError> {
    let Some(row) = fetch_plan_row(conn, id)? else {
        return Ok(None);
    };
    let mut problems = Problems { found: Vec::new() };
    let seal = text(&row, "content_hash").unwrap_or_default();
    if plan_seal_of(id, &row) != seal {
        problems.add(
            "manual_exercise_plans",
            id,
            None,
            "the stored seal is not the seal of the plan: a column of the plan was changed \
             after it was sealed"
                .into(),
        );
    }
    let audit = crate::query_json_rows_bound(conn, Trail::Plan.rows_sql(), &[id])?;
    let legacy = check_trail(Trail::Plan.table(), id, &audit, seal, &mut problems);
    let signoffs = crate::query_json_rows_bound(conn, SIGNOFFS_SQL, &["plan", id])?;
    check_signoffs(&signoffs, &seals_of(&audit), &mut problems);
    Ok(Some(verdict(problems, None, legacy)))
}

impl Reader {
    /// Check everything the lake holds about manual record `id` again: its
    /// seal, its audit trail, its reviewer, its sign-offs, its attachments
    /// and their stored files. `None` when there is no such record.
    ///
    /// # Errors
    /// Returns an error if a query fails.
    pub fn verify_manual_record(&self, id: &str) -> Result<Option<AuditVerdict>, StoreError> {
        verify_record(&self.conn, id, &attachments_dir(&self.path))
    }

    /// Check everything the lake holds about plan `id` again: its seal, its
    /// audit trail and its sign-offs. `None` when there is no such plan.
    ///
    /// # Errors
    /// Returns an error if a query fails.
    pub fn verify_manual_plan(&self, id: &str) -> Result<Option<AuditVerdict>, StoreError> {
        verify_plan(&self.conn, id)
    }
}
