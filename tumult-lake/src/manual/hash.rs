//! Content hashing (canonical JSON → SHA-256), ULID generation, and the
//! row-fetch helpers the lifecycle writer and the rehash path share.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::ManualError;
use crate::StoreError;

/// Canonical serialization of the content fields, hashed into
/// `content_hash`. Field order is fixed (serde struct order) so the hash is
/// stable for identical content.
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

pub(crate) fn content_hash(content: &CanonicalContent<'_>) -> String {
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

/// Recompute the content hash after a lifecycle/status change by reading
/// the row back and re-serializing the canonical fields.
pub(crate) fn rehash(conn: &Connection, id: &str) -> Result<String, ManualError> {
    let row = fetch_row(conn, id)?.ok_or_else(|| ManualError::NotFound(id.to_string()))?;
    let s = |k: &str| row.get(k).and_then(serde_json::Value::as_str).unwrap_or("");
    let opt_s = |k: &str| row.get(k).and_then(serde_json::Value::as_str);
    let action_items: Vec<String> = row
        .get("action_items")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let framework_refs: Vec<String> = row
        .get("framework_refs")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    let canonical = CanonicalContent {
        experiment_name: s("experiment_name"),
        exercise_type: s("exercise_type"),
        executed_at_ns: row
            .get("executed_at_ns")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(0),
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
        recovery_time_s: row
            .get("recovery_time_s")
            .and_then(serde_json::Value::as_f64),
        duration_s: row.get("duration_s").and_then(serde_json::Value::as_f64),
        entered_by: s("entered_by"),
        attestation: s("attestation"),
        renewal_due_ns: row
            .get("renewal_due_ns")
            .and_then(serde_json::Value::as_i64),
        framework_refs: &framework_refs,
        status: s("status"),
    };
    Ok(content_hash(&canonical))
}
