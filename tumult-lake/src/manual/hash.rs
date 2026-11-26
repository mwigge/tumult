//! Content hashing (canonical JSON → SHA-256), ULID generation, and the
//! row-fetch helpers the lifecycle writer and the rehash path share.
//!
//! A record is sealed in the form of its `hash_version`. Version 1 is
//! [`CanonicalContent`] alone, the form of every row sealed before schema
//! v22. Version 2 ([`SealV2`]) holds version 1 and adds the id, the entry
//! time, the DR facts, the steps, the actions, the links and the reviewer,
//! and, when they are set, the severities, the tester, the TLPT block, the
//! retests and the plan scenarios.
//! Every lifecycle write seals the row again in version 2; the audit verify
//! recomputes a stored seal in the version it was made in.
//!
//! A plan is sealed the same way, in the one form of [`PlanSeal`]: every
//! column of the plan but the seal itself.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::{params, Connection};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::reader::{ACTIONS_SQL, LINKS_SQL, STEPS_SQL};
use super::{ManualError, PlanError};
use crate::sql;
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
/// `closed_by`) change after sign-off and are left out. The fields from
/// `severity` on came later: they are left out when empty, so an action
/// sealed before them keeps its seal.
#[derive(Serialize, Deserialize)]
struct ActionSeal {
    ordinal: i64,
    title: String,
    owner: String,
    due_at_ns: Option<i64>,
    ticket: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    severity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retest_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retest_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct LinkSeal {
    ordinal: i64,
    kind: String,
    linked_id: String,
    label: Option<String>,
}

/// The version 2 form, in this fixed order. The fields from
/// `findings_severity` on came with the gaps of the DORA report: they are
/// left out when empty, so a record sealed before them keeps its seal.
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
    #[serde(skip_serializing_if = "Option::is_none")]
    findings_severity: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tester: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tester_kind: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tlpt_attestation_ref: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tlpt_attested_at_ns: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tlpt_summary_ref: Option<&'a str>,
    /// The plan scenarios (schema v26), left out when there are none, so
    /// a record sealed before them keeps its seal.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    scenarios: Vec<String>,
}

/// The seal of a plan, in this fixed order: never reorder a field.
#[derive(Serialize)]
struct PlanSeal<'a> {
    id: &'a str,
    name: &'a str,
    exercise_type: &'a str,
    target_system: Option<&'a str>,
    target_environment: Option<&'a str>,
    function_id: Option<&'a str>,
    scope: Option<&'a str>,
    owner: &'a str,
    planned_at_ns: Option<i64>,
    due_at_ns: i64,
    interval_days: Option<i64>,
    rto_target_s: Option<f64>,
    rpo_target_s: Option<f64>,
    status: &'a str,
    created_by: &'a str,
    created_at_ns: i64,
    updated_by: Option<&'a str>,
    updated_at_ns: Option<i64>,
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

/// The first row `sql` reads for `id`, as a JSON object (column → value).
fn fetch_one(
    conn: &Connection,
    sql: &str,
    id: &str,
) -> Result<Option<serde_json::Value>, StoreError> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query_map(params![id], |r| r.get::<usize, String>(0))?;
    match rows.next() {
        None => Ok(None),
        Some(row) => Ok(Some(serde_json::from_str(&row?)?)),
    }
}

/// One row of the table as a JSON object (column → value).
pub(crate) fn fetch_row(
    conn: &Connection,
    id: &str,
) -> Result<Option<serde_json::Value>, StoreError> {
    fetch_one(conn, sql::manual::RECORD_ROW, id)
}

/// One plan as a JSON object (column → value).
pub(crate) fn fetch_plan_row(
    conn: &Connection,
    id: &str,
) -> Result<Option<serde_json::Value>, StoreError> {
    fetch_one(conn, sql::manual::PLAN_ROW, id)
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
    Ok(seal_of(conn, id, &row, HASH_VERSION)?)
}

/// The seal of record `id`, stored as `row`, in the form of `version`: 1
/// is [`CanonicalContent`] alone, 2 is [`SealV2`] with the child rows read
/// back. The audit verify recomputes a stored seal with it.
pub(crate) fn seal_of(
    conn: &Connection,
    id: &str,
    row: &serde_json::Value,
    version: i64,
) -> Result<String, StoreError> {
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
    if version == 1 {
        return Ok(content_hash(&content));
    }
    let seal = SealV2 {
        hash_version: version,
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
        participants: json_list(row, "participants")?,
        steps: child_rows(conn, STEPS_SQL, id)?,
        actions: child_rows(conn, ACTIONS_SQL, id)?,
        links: child_rows(conn, LINKS_SQL, id)?,
        reviewed_by: opt_s("reviewed_by"),
        reviewed_at_ns: int("reviewed_at_ns"),
        review_note: opt_s("review_note"),
        findings_severity: opt_s("findings_severity"),
        tester: opt_s("tester"),
        tester_kind: opt_s("tester_kind"),
        tlpt_attestation_ref: opt_s("tlpt_attestation_ref"),
        tlpt_attested_at_ns: int("tlpt_attested_at_ns"),
        tlpt_summary_ref: opt_s("tlpt_summary_ref"),
        scenarios: json_list(row, "scenarios")?,
    };
    Ok(content_hash(&seal))
}

/// Seal record `id` again in the current form: store its new hash and the
/// version of the form, and return the hash.
pub(crate) fn reseal(conn: &Connection, id: &str) -> Result<String, ManualError> {
    let hash = rehash(conn, id)?;
    conn.execute(sql::manual::RESEAL_RECORD, params![hash, HASH_VERSION, id])?;
    Ok(hash)
}

/// Recompute the seal of plan `id` from the stored row.
pub(crate) fn rehash_plan(conn: &Connection, id: &str) -> Result<String, ManualError> {
    let row = fetch_plan_row(conn, id)?.ok_or_else(|| PlanError::NotFound(id.to_string()))?;
    Ok(plan_seal_of(id, &row))
}

/// The seal of plan `id`, stored as `row`.
pub(crate) fn plan_seal_of(id: &str, row: &serde_json::Value) -> String {
    let s = |k: &str| row.get(k).and_then(serde_json::Value::as_str).unwrap_or("");
    let opt_s = |k: &str| row.get(k).and_then(serde_json::Value::as_str);
    let num = |k: &str| row.get(k).and_then(serde_json::Value::as_f64);
    let int = |k: &str| row.get(k).and_then(serde_json::Value::as_i64);
    let seal = PlanSeal {
        id,
        name: s("name"),
        exercise_type: s("exercise_type"),
        target_system: opt_s("target_system"),
        target_environment: opt_s("target_environment"),
        function_id: opt_s("function_id"),
        scope: opt_s("scope"),
        owner: s("owner"),
        planned_at_ns: int("planned_at_ns"),
        due_at_ns: int("due_at_ns").unwrap_or(0),
        interval_days: int("interval_days"),
        rto_target_s: num("rto_target_s"),
        rpo_target_s: num("rpo_target_s"),
        status: s("status"),
        created_by: s("created_by"),
        created_at_ns: int("created_at_ns").unwrap_or(0),
        updated_by: opt_s("updated_by"),
        updated_at_ns: int("updated_at_ns"),
    };
    content_hash(&seal)
}

/// Seal plan `id` again from what is stored, and return the seal.
pub(crate) fn reseal_plan(conn: &Connection, id: &str) -> Result<String, ManualError> {
    let hash = rehash_plan(conn, id)?;
    conn.execute(sql::manual::RESEAL_PLAN, params![hash, id])?;
    Ok(hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manual::{
        ActionItem, ExerciseDetail, ExerciseLink, ExerciseStep, ExerciseType, LinkKind,
        ManualOutcome, NewExercisePlan, NewManualExperiment, Participant, PlanEdit, Retest,
        RetestKind, Severity, StepResult, Tester, TesterKind,
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
                    severity: Some(Severity::High),
                    retest: Some(Retest {
                        kind: RetestKind::Run,
                        id: "exp-retest".into(),
                    }),
                }],
                links: vec![ExerciseLink {
                    kind: LinkKind::Function,
                    linked_id: "cif-0001".into(),
                    label: None,
                }],
                plan_id: None,
                findings_severity: Some(Severity::Medium),
                tester: Some(Tester {
                    name: "Red Team Ltd".into(),
                    kind: TesterKind::External,
                }),
                tlpt: None,
                scenarios: Vec::new(),
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
            "UPDATE manual_experiments SET findings_severity = 'low' WHERE id = ?",
            "UPDATE manual_experiments SET tester = 'mallory' WHERE id = ?",
            "UPDATE manual_experiments SET tester_kind = 'internal' WHERE id = ?",
            "UPDATE manual_experiments SET tlpt_attestation_ref = 'ATT-9' WHERE id = ?",
            "UPDATE manual_experiments SET tlpt_attested_at_ns = 5 WHERE id = ?",
            "UPDATE manual_experiments SET tlpt_summary_ref = 'TSR-9' WHERE id = ?",
            "UPDATE manual_action_items SET severity = 'low' WHERE experiment_id = ?",
            "UPDATE manual_action_items SET retest_kind = 'record' WHERE experiment_id = ?",
            "UPDATE manual_action_items SET retest_id = 'other' WHERE experiment_id = ?",
            "UPDATE manual_experiments SET scenarios = ['insider_attack'] WHERE id = ?",
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

    /// A record of fixed content with an action, and none of the fields the
    /// DORA gap report added (severity, tester, TLPT block, retest): its
    /// version 2 seal is the one it had before those fields existed.
    #[test]
    fn a_record_without_the_dora_gap_fields_keeps_the_seal_it_had() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let conn = &writer.conn;
        conn.execute_batch(
            "INSERT INTO manual_experiments (id, experiment_name, exercise_type, \
             executed_at_ns, hypothesis, method, outcome_status, entered_by, entered_at_ns, \
             attestation, status, framework_refs, content_hash, hash_version) VALUES \
             ('m-fixed', 'drill', 'drill', 1000, 'h', 'm', 'passed', 'alice', 2000, 'a', \
             'draft', ['dora.art24.6'], '', 2); \
             INSERT INTO manual_action_items (id, experiment_id, ordinal, title, owner) \
             VALUES ('a-1', 'm-fixed', 1, 'tune', 'carol');",
        )
        .unwrap();
        let row = fetch_row(conn, "m-fixed").unwrap().unwrap();
        assert_eq!(
            seal_of(conn, "m-fixed", &row, 2).unwrap(),
            "f32b563b19ca02554b77d8dcb277d3b9f6603729a879285640f6dda6b0a1f5bf"
        );
    }

    /// Run `sql` on the plan and give the hash it would have then, leaving
    /// the row as it was.
    fn plan_hash_after(conn: &Connection, id: &str, sql: &str) -> String {
        conn.execute_batch("BEGIN TRANSACTION").unwrap();
        conn.execute(sql, params![id]).unwrap();
        let hash = rehash_plan(conn, id).unwrap();
        conn.execute_batch("ROLLBACK").unwrap();
        hash
    }

    #[test]
    fn a_direct_edit_of_any_plan_column_breaks_the_plan_seal() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(&dir.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        let plan = NewExercisePlan {
            name: "db failover".into(),
            exercise_type: ExerciseType::Failover,
            target_system: Some("db".into()),
            target_environment: Some("prod".into()),
            function_id: Some("cif-0001".into()),
            scope: Some("promote the standby".into()),
            owner: "alice".into(),
            planned_at_ns: Some(1_000),
            due_at_ns: 2_000,
            interval_days: Some(365),
            rto_target_s: Some(900.0),
            rpo_target_s: Some(60.0),
        };
        let id = writer.create_manual_plan(&plan, "alice").unwrap();
        let edit = PlanEdit {
            owner: Some("carol".into()),
            ..PlanEdit::default()
        };
        writer.edit_manual_plan(&id, &edit, "bob").unwrap();
        let conn = &writer.conn;
        let stored = fetch_plan_row(conn, &id).unwrap().unwrap()["content_hash"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(rehash_plan(conn, &id).unwrap(), stored);

        let edits = [
            "UPDATE manual_exercise_plans SET name = 'other' WHERE id = ?",
            "UPDATE manual_exercise_plans SET exercise_type = 'restore' WHERE id = ?",
            "UPDATE manual_exercise_plans SET target_system = 'cache' WHERE id = ?",
            "UPDATE manual_exercise_plans SET target_environment = 'dev' WHERE id = ?",
            "UPDATE manual_exercise_plans SET function_id = NULL WHERE id = ?",
            "UPDATE manual_exercise_plans SET scope = 'less' WHERE id = ?",
            "UPDATE manual_exercise_plans SET owner = 'mallory' WHERE id = ?",
            "UPDATE manual_exercise_plans SET planned_at_ns = 1001 WHERE id = ?",
            "UPDATE manual_exercise_plans SET due_at_ns = 9999 WHERE id = ?",
            "UPDATE manual_exercise_plans SET interval_days = 730 WHERE id = ?",
            "UPDATE manual_exercise_plans SET rto_target_s = 901 WHERE id = ?",
            "UPDATE manual_exercise_plans SET rpo_target_s = 61 WHERE id = ?",
            "UPDATE manual_exercise_plans SET status = 'cancelled' WHERE id = ?",
            "UPDATE manual_exercise_plans SET created_by = 'mallory' WHERE id = ?",
            "UPDATE manual_exercise_plans SET created_at_ns = created_at_ns + 1 WHERE id = ?",
            "UPDATE manual_exercise_plans SET updated_by = 'mallory' WHERE id = ?",
            "UPDATE manual_exercise_plans SET updated_at_ns = updated_at_ns + 1 WHERE id = ?",
        ];
        for sql in edits {
            assert_ne!(plan_hash_after(conn, &id, sql), stored, "not sealed: {sql}");
        }
    }
}
