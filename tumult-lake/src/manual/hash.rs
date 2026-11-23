//! Content hashing (canonical JSON → SHA-256), ULID generation, and the
//! row-fetch helpers the lifecycle writer and the rehash path share.
//!
//! A record is sealed in the form of its `hash_version`. Version 1 is
//! [`CanonicalContent`] alone, the form of every row sealed before schema
//! v22. Version 2 ([`SealV2`]) holds version 1 and adds the id, the entry
//! time, the DR facts, the steps, the actions, the links and the reviewer.
//! Every lifecycle write seals the row again in version 2.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::{params, Connection};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::reader::{ACTIONS_SQL, LINKS_SQL, STEPS_SQL};
use super::ManualError;
use crate::StoreError;

/// The form new seals are computed in.
pub(crate) const HASH_VERSION: i64 = 2;

/// Canonical serialization of the content fields, hashed into
/// `content_hash`. Field order is fixed (serde struct order) so the hash is
/// stable for identical content. This is the whole of the version 1 form.
#[derive(Serialize)]
pub(crate) struct CanonicalContent<'a> {
    pub(crate) experiment_name: &'a str,
    pub(crate) exercise_type: &'a str,
    pub(crate) executed_at_ns: i64,
    pub(crate) hypothesis: &'a str,
    pub(crate) method: &'a str,
    pub(crate) outcome_status: &'a str,
    pub(crate) hypothesis_met: Option<bool>,
    pub(crate) findings: Option<&'a str>,
    pub(crate) action_items: &'a [String],
    pub(crate) target_system: Option<&'a str>,
    pub(crate) target_environment: Option<&'a str>,
    pub(crate) blast_radius: Option<&'a str>,
    pub(crate) recovery_time_s: Option<f64>,
    pub(crate) duration_s: Option<f64>,
    pub(crate) entered_by: &'a str,
    pub(crate) attestation: &'a str,
    pub(crate) renewal_due_ns: Option<i64>,
    pub(crate) framework_refs: &'a [String],
    pub(crate) status: &'a str,
}

// The parts of the version 2 form read from the child rows. Their field
// order is part of the form: never reorder a field.

#[derive(Serialize, Deserialize)]
struct ParticipantSeal {
    name: String,
    role: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct StepSeal {
    ordinal: i64,
    title: String,
    actor: Option<String>,
    started_at_ns: Option<i64>,
    ended_at_ns: Option<i64>,
    result: Option<String>,
    note: Option<String>,
}

/// An action as entered. Its tracking columns (`status`, `closed_at_ns`,
/// `closed_by`) change after sign-off and are left out.
#[derive(Serialize, Deserialize)]
struct ActionSeal {
    ordinal: i64,
    title: String,
    owner: String,
    due_at_ns: Option<i64>,
    ticket: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct LinkSeal {
    ordinal: i64,
    kind: String,
    linked_id: String,
    label: Option<String>,
}

/// The version 2 form, in this fixed order.
#[derive(Serialize)]
struct SealV2<'a> {
    hash_version: i64,
    id: &'a str,
    #[serde(flatten)]
    content: CanonicalContent<'a>,
    entered_at_ns: i64,
    rto_target_s: Option<f64>,
    rpo_target_s: Option<f64>,
    rpo_measured_s: Option<f64>,
    data_verified: Option<bool>,
    plan_id: Option<&'a str>,
    participants: Vec<ParticipantSeal>,
    steps: Vec<StepSeal>,
    actions: Vec<ActionSeal>,
    links: Vec<LinkSeal>,
    reviewed_by: Option<&'a str>,
    reviewed_at_ns: Option<i64>,
    review_note: Option<&'a str>,
}

pub(crate) fn content_hash<T: Serialize>(content: &T) -> String {
    let json = serde_json::to_string(content).unwrap_or_default();
    let digest = Sha256::digest(json.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// The timestamp-and-randomness state of the monotonic ULID generator.
static ULID_STATE: Mutex<(u64, u128)> = Mutex::new((0, 0));

/// Crockford base32 alphabet (no I, L, O, U).
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Generate a ULID: 48-bit unix-millis + 80 bits of randomness, Crockford
/// base32, 26 chars. Monotonic within one process (the random part
/// increments while the millisecond is unchanged). Randomness comes from
/// `/dev/urandom`, falling back to a time/pid mix.
pub(crate) fn ulid() -> String {
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let mut state = ULID_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let random = if state.0 == now_ms {
        state.1 = state.1.wrapping_add(1) & ((1 << 80) - 1);
        state.1
    } else {
        let fresh = random_u80(now_ms);
        *state = (now_ms, fresh);
        fresh
    };
    let value = ((now_ms as u128) << 80) | random;
    let mut out = String::with_capacity(26);
    for i in (0..26).rev() {
        out.push(CROCKFORD[((value >> (5 * i)) & 31) as usize] as char);
    }
    out
}

fn random_u80(salt: u64) -> u128 {
    use std::io::Read;
    let mut buf = [0u8; 16];
    let from_urandom = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut buf))
        .is_ok();
    let mixed: u128 = if from_urandom {
        u128::from_ne_bytes(buf)
    } else {
        // Fallback: time + pid + salt, spread across the 128-bit space.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        (nanos ^ ((std::process::id() as u128) << 64) ^ ((salt as u128) << 96))
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
    };
    mixed & ((1 << 80) - 1)
}

pub(crate) fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

/// One row of the table as a JSON object (column → value).
pub(crate) fn fetch_row(
    conn: &Connection,
    id: &str,
) -> Result<Option<serde_json::Value>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT row_to_json(t) AS j FROM \
         (SELECT * FROM manual_experiments WHERE id = ?) AS t",
    )?;
    let mut rows = stmt.query_map(params![id], |r| r.get::<usize, String>(0))?;
    match rows.next() {
        None => Ok(None),
        Some(row) => Ok(Some(serde_json::from_str(&row?)?)),
    }
}

pub(crate) fn status_of(row: &serde_json::Value) -> String {
    row.get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The child rows of record `id` that `sql` reads, each as a `T`.
fn child_rows<T: DeserializeOwned>(
    conn: &Connection,
    sql: &str,
    id: &str,
) -> Result<Vec<T>, StoreError> {
    crate::query_json_rows_bound(conn, sql, &[id])?
        .into_iter()
        .map(|row| serde_json::from_value(row).map_err(StoreError::from))
        .collect()
}

/// The JSON list in column `key` of `row`; empty when the column is NULL.
fn json_list<T: DeserializeOwned>(
    row: &serde_json::Value,
    key: &str,
) -> Result<Vec<T>, StoreError> {
    match row.get(key) {
        None | Some(serde_json::Value::Null) => Ok(Vec::new()),
        Some(list) => Ok(serde_json::from_value(list.clone())?),
    }
}

/// Recompute the content hash of record `id` in the current form, by
/// reading the row and its child rows back.
pub(crate) fn rehash(conn: &Connection, id: &str) -> Result<String, ManualError> {
    let row = fetch_row(conn, id)?.ok_or_else(|| ManualError::NotFound(id.to_string()))?;
    let s = |k: &str| row.get(k).and_then(serde_json::Value::as_str).unwrap_or("");
    let opt_s = |k: &str| row.get(k).and_then(serde_json::Value::as_str);
    let num = |k: &str| row.get(k).and_then(serde_json::Value::as_f64);
    let int = |k: &str| row.get(k).and_then(serde_json::Value::as_i64);
    let action_items: Vec<String> = row
        .get("action_items")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let framework_refs: Vec<String> = row
        .get("framework_refs")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let content = CanonicalContent {
        experiment_name: s("experiment_name"),
        exercise_type: s("exercise_type"),
        executed_at_ns: int("executed_at_ns").unwrap_or(0),
        hypothesis: s("hypothesis"),
        method: s("method"),
        outcome_status: s("outcome_status"),
        hypothesis_met: row
            .get("hypothesis_met")
            .and_then(serde_json::Value::as_bool),
        findings: opt_s("findings"),
        action_items: &action_items,
        target_system: opt_s("target_system"),
        target_environment: opt_s("target_environment"),
        blast_radius: opt_s("blast_radius"),
        recovery_time_s: num("recovery_time_s"),
        duration_s: num("duration_s"),
        entered_by: s("entered_by"),
        attestation: s("attestation"),
        renewal_due_ns: int("renewal_due_ns"),
        framework_refs: &framework_refs,
        status: s("status"),
    };
    let seal = SealV2 {
        hash_version: HASH_VERSION,
        id,
        content,
        entered_at_ns: int("entered_at_ns").unwrap_or(0),
        rto_target_s: num("rto_target_s"),
        rpo_target_s: num("rpo_target_s"),
        rpo_measured_s: num("rpo_measured_s"),
        data_verified: row
            .get("data_verified")
            .and_then(serde_json::Value::as_bool),
        plan_id: opt_s("plan_id"),
        participants: json_list(&row, "participants")?,
        steps: child_rows(conn, STEPS_SQL, id)?,
        actions: child_rows(conn, ACTIONS_SQL, id)?,
        links: child_rows(conn, LINKS_SQL, id)?,
        reviewed_by: opt_s("reviewed_by"),
        reviewed_at_ns: int("reviewed_at_ns"),
        review_note: opt_s("review_note"),
    };
    Ok(content_hash(&seal))
}

/// Seal record `id` again in the current form: store its new hash and the
/// version of the form, and return the hash.
pub(crate) fn reseal(conn: &Connection, id: &str) -> Result<String, ManualError> {
    let hash = rehash(conn, id)?;
    conn.execute(
        "UPDATE manual_experiments SET content_hash = ?, hash_version = ? WHERE id = ?",
        params![hash, HASH_VERSION, id],
    )?;
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::{
        ActionItem, ExerciseDetail, ExerciseLink, ExerciseStep, ExerciseType, LinkKind,
        ManualOutcome, NewManualExperiment, Participant, StepResult,
    };
    use crate::Store;

    fn restore() -> NewManualExperiment {
        NewManualExperiment {
            experiment_name: "db-restore".into(),
            exercise_type: ExerciseType::Restore,
            executed_at_ns: 1_000,
            hypothesis: "h".into(),
            method: "m".into(),
            outcome: ManualOutcome::Passed,
            hypothesis_met: Some(true),
            findings: Some("f".into()),
            action_items: vec![],
            target_system: Some("db".into()),
            target_environment: Some("prod".into()),
            blast_radius: None,
            recovery_time_s: Some(30.0),
            duration_s: None,
            entered_by: "alice".into(),
            attestation: "a".into(),
            renewal_due_ns: None,
            framework_refs: vec![],
            exercise: ExerciseDetail {
                rto_target_s: Some(60.0),
                rpo_target_s: Some(5.0),
                rpo_measured_s: Some(2.0),
                data_verified: Some(true),
                participants: vec![Participant {
                    name: "alice".into(),
                    role: Some("lead".into()),
                }],
                steps: vec![ExerciseStep {
                    title: "restore".into(),
                    actor: Some("alice".into()),
                    started_at_ns: Some(1_000),
                    ended_at_ns: Some(2_000),
                    result: Some(StepResult::Done),
                    note: None,
                }],
                actions: vec![ActionItem {
                    title: "tune".into(),
                    owner: "carol".into(),
                    due_at_ns: Some(9_000),
                    ticket: None,
                }],
                links: vec![ExerciseLink {
                    kind: LinkKind::Function,
                    linked_id: "cif-0001".into(),
                    label: None,
                }],
            },
        }
    }

    /// Run `sql` on the record and give the hash it would have then,
    /// leaving the row as it was.
    fn hash_after(conn: &Connection, id: &str, sql: &str) -> String {
        conn.execute_batch("BEGIN TRANSACTION").unwrap();
        conn.execute(sql, params![id]).unwrap();
        let hash = rehash(conn, id).unwrap();
        conn.execute_batch("ROLLBACK").unwrap();
        hash
    }

    #[test]
    fn the_seal_covers_the_reviewer_and_the_dr_facts_but_not_the_action_tracking() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let id = writer.create_manual_draft(&restore()).unwrap();
        writer.submit_manual(&id, None, "alice").unwrap();
        writer.verify_manual(&id, "bob", Some("checked")).unwrap();
        let conn = &writer.conn;
        let stored = fetch_row(conn, &id).unwrap().unwrap()["content_hash"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(rehash(conn, &id).unwrap(), stored);

        let covered = [
            "UPDATE manual_experiments SET reviewed_by = 'mallory' WHERE id = ?",
            "UPDATE manual_experiments SET reviewed_at_ns = reviewed_at_ns + 1 WHERE id = ?",
            "UPDATE manual_experiments SET review_note = 'fine' WHERE id = ?",
            "UPDATE manual_experiments SET entered_at_ns = entered_at_ns + 1 WHERE id = ?",
            "UPDATE manual_experiments SET findings = 'other' WHERE id = ?",
            "UPDATE manual_experiments SET rto_target_s = 61 WHERE id = ?",
            "UPDATE manual_experiments SET rpo_target_s = 6 WHERE id = ?",
            "UPDATE manual_experiments SET rpo_measured_s = 3 WHERE id = ?",
            "UPDATE manual_experiments SET data_verified = FALSE WHERE id = ?",
            "UPDATE manual_experiments SET participants = '[]' WHERE id = ?",
            "UPDATE manual_experiments SET plan_id = 'plan-1' WHERE id = ?",
            "UPDATE manual_exercise_steps SET title = 'skip' WHERE experiment_id = ?",
            "UPDATE manual_exercise_steps SET result = 'failed' WHERE experiment_id = ?",
            "UPDATE manual_action_items SET owner = 'dave' WHERE experiment_id = ?",
            "UPDATE manual_action_items SET due_at_ns = 9001 WHERE experiment_id = ?",
            "UPDATE manual_experiment_links SET linked_id = 'cif-0002' WHERE experiment_id = ?",
            "UPDATE manual_experiment_links SET kind = 'entity' WHERE experiment_id = ?",
            "DELETE FROM manual_exercise_steps WHERE experiment_id = ?",
        ];
        for sql in covered {
            assert_ne!(hash_after(conn, &id, sql), stored, "not sealed: {sql}");
        }
        let left_out = [
            "UPDATE manual_action_items SET status = 'done', closed_at_ns = 5, \
             closed_by = 'carol' WHERE experiment_id = ?",
            "UPDATE manual_experiments SET batch_id = 'b-1' WHERE id = ?",
        ];
        for sql in left_out {
            assert_eq!(hash_after(conn, &id, sql), stored, "sealed: {sql}");
        }
    }
}
