//! Manual evidence: hand-entered test records with attestation and review.
//!
//! Complements the automated (OTLP-ingested) experiment telemetry with
//! records of manually executed resilience tests — game days, tabletop
//! exercises, failover drills, pentests — that never touch an agent.
//!
//! Lifecycle: `draft` (fully mutable) → `submitted` (locked; requires a
//! non-empty attestation) → `verified` / `rejected` by a reviewer who must
//! NOT be the person who entered the record (segregation of duties, after
//! DORA Art. 24(7) / ISO 27001 A.5.35). Every mutation appends an
//! audit row whose `prev_hash`/`new_hash` chain the record's `content_hash`
//! (SHA-256 over the canonical JSON content), making silent edits
//! tamper-evident. `verified` records score exactly like automated runs;
//! `draft`/`submitted` records count toward coverage as "pending
//! verification" with zero score weight; `inconclusive` outcomes are
//! excluded from scoring entirely.

use std::fmt;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use duckdb::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{with_tx, Reader, StoreError, Writer};

/// Exercise kinds for a manual test record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExerciseType {
    GameDay,
    Tabletop,
    Failover,
    Pentest,
    Drill,
    Other,
}

impl ExerciseType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GameDay => "gameday",
            Self::Tabletop => "tabletop",
            Self::Failover => "failover",
            Self::Pentest => "pentest",
            Self::Drill => "drill",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Result<Self, ManualError> {
        Ok(match s {
            "gameday" => Self::GameDay,
            "tabletop" => Self::Tabletop,
            "failover" => Self::Failover,
            "pentest" => Self::Pentest,
            "drill" => Self::Drill,
            "other" => Self::Other,
            other => {
                return Err(ManualError::Invalid(format!(
                    "unknown exercise_type '{other}' \
                     (gameday|tabletop|failover|pentest|drill|other)"
                )))
            }
        })
    }
}

/// Outcome of a manually executed test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualOutcome {
    Passed,
    Failed,
    Partial,
    Inconclusive,
}

impl ManualOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Partial => "partial",
            Self::Inconclusive => "inconclusive",
        }
    }

    pub fn parse(s: &str) -> Result<Self, ManualError> {
        Ok(match s {
            "passed" => Self::Passed,
            "failed" => Self::Failed,
            "partial" => Self::Partial,
            "inconclusive" => Self::Inconclusive,
            other => {
                return Err(ManualError::Invalid(format!(
                    "unknown outcome_status '{other}' (passed|failed|partial|inconclusive)"
                )))
            }
        })
    }
}

/// Evidence attachment kinds. The API currently accepts `url` and `ticket`
/// only (no file storage); `file` and `log_excerpt` are reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    File,
    Url,
    LogExcerpt,
    Ticket,
}

impl AttachmentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Url => "url",
            Self::LogExcerpt => "log_excerpt",
            Self::Ticket => "ticket",
        }
    }

    pub fn parse(s: &str) -> Result<Self, ManualError> {
        Ok(match s {
            "file" => Self::File,
            "url" => Self::Url,
            "log_excerpt" => Self::LogExcerpt,
            "ticket" => Self::Ticket,
            other => {
                return Err(ManualError::Invalid(format!(
                    "unknown attachment kind '{other}' (file|url|log_excerpt|ticket)"
                )))
            }
        })
    }
}

/// Errors from the manual-evidence lifecycle. The API layer maps:
/// `Invalid`/`SelfReview` → 400, `NotFound` → 404, `WrongStatus` → 409,
/// `Store` → 500.
#[derive(Debug)]
pub enum ManualError {
    /// Bad input (unknown enum value, empty required field).
    Invalid(String),
    /// No manual experiment with this id.
    NotFound(String),
    /// The record's current status does not allow this action.
    WrongStatus { status: String, action: String },
    /// Reviewer must differ from the person who entered the record.
    SelfReview,
    /// Underlying store failure.
    Store(StoreError),
}

impl fmt::Display for ManualError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(msg) => write!(f, "invalid manual experiment: {msg}"),
            Self::NotFound(id) => write!(f, "manual experiment '{id}' not found"),
            Self::WrongStatus { status, action } => {
                write!(f, "cannot {action} a record in status '{status}'")
            }
            Self::SelfReview => {
                write!(
                    f,
                    "reviewer must differ from the person who entered the record"
                )
            }
            Self::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for ManualError {}

impl From<StoreError> for ManualError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

impl From<duckdb::Error> for ManualError {
    fn from(e: duckdb::Error) -> Self {
        Self::Store(StoreError::from(e))
    }
}

/// Content of a new (or fully replaced) manual test record.
/// `entered_by`/`entered_at_ns` are set on creation only.
#[derive(Debug, Clone)]
pub struct NewManualExperiment {
    pub experiment_name: String,
    pub exercise_type: ExerciseType,
    pub executed_at_ns: i64,
    pub hypothesis: String,
    pub method: String,
    pub outcome: ManualOutcome,
    pub hypothesis_met: Option<bool>,
    pub findings: Option<String>,
    pub action_items: Vec<String>,
    pub target_system: Option<String>,
    pub target_environment: Option<String>,
    pub blast_radius: Option<String>,
    pub recovery_time_s: Option<f64>,
    pub duration_s: Option<f64>,
    pub entered_by: String,
    pub attestation: String,
    pub renewal_due_ns: Option<i64>,
    pub framework_refs: Vec<String>,
}

impl NewManualExperiment {
    fn validate(&self) -> Result<(), ManualError> {
        for (field, value) in [
            ("experiment_name", &self.experiment_name),
            ("hypothesis", &self.hypothesis),
            ("method", &self.method),
            ("entered_by", &self.entered_by),
            ("attestation", &self.attestation),
        ] {
            if value.trim().is_empty() {
                return Err(ManualError::Invalid(format!("'{field}' must not be empty")));
            }
        }
        if self.executed_at_ns <= 0 {
            return Err(ManualError::Invalid(
                "'executed_at_ns' must be a positive unix-nanos timestamp".into(),
            ));
        }
        Ok(())
    }
}

/// Canonical serialization of the content fields, hashed into
/// `content_hash`. Field order is fixed (serde struct order) so the hash is
/// stable for identical content.
#[derive(Serialize)]
struct CanonicalContent<'a> {
    experiment_name: &'a str,
    exercise_type: &'a str,
    executed_at_ns: i64,
    hypothesis: &'a str,
    method: &'a str,
    outcome_status: &'a str,
    hypothesis_met: Option<bool>,
    findings: Option<&'a str>,
    action_items: &'a [String],
    target_system: Option<&'a str>,
    target_environment: Option<&'a str>,
    blast_radius: Option<&'a str>,
    recovery_time_s: Option<f64>,
    duration_s: Option<f64>,
    entered_by: &'a str,
    attestation: &'a str,
    renewal_due_ns: Option<i64>,
    framework_refs: &'a [String],
    status: &'a str,
}

fn content_hash(content: &CanonicalContent<'_>) -> String {
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
fn ulid() -> String {
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

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

/// One row of the table as a JSON object (column → value).
fn fetch_row(conn: &Connection, id: &str) -> Result<Option<serde_json::Value>, StoreError> {
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

fn status_of(row: &serde_json::Value) -> String {
    row.get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Insert the experiment row (no transaction; caller wraps).
fn insert_draft(
    conn: &Connection,
    new: &NewManualExperiment,
    batch_id: Option<&str>,
) -> Result<(String, String), ManualError> {
    let id = ulid();
    let canonical = CanonicalContent {
        experiment_name: &new.experiment_name,
        exercise_type: new.exercise_type.as_str(),
        executed_at_ns: new.executed_at_ns,
        hypothesis: &new.hypothesis,
        method: &new.method,
        outcome_status: new.outcome.as_str(),
        hypothesis_met: new.hypothesis_met,
        findings: new.findings.as_deref(),
        action_items: &new.action_items,
        target_system: new.target_system.as_deref(),
        target_environment: new.target_environment.as_deref(),
        blast_radius: new.blast_radius.as_deref(),
        recovery_time_s: new.recovery_time_s,
        duration_s: new.duration_s,
        entered_by: &new.entered_by,
        attestation: &new.attestation,
        renewal_due_ns: new.renewal_due_ns,
        framework_refs: &new.framework_refs,
        status: "draft",
    };
    let hash = content_hash(&canonical);
    conn.execute(
        "INSERT INTO manual_experiments (
            id, experiment_name, exercise_type, executed_at_ns, hypothesis, method,
            outcome_status, hypothesis_met, findings, action_items,
            target_system, target_environment, blast_radius,
            recovery_time_s, duration_s, origin,
            entered_by, entered_at_ns, attestation, status,
            renewal_due_ns, framework_refs, batch_id, content_hash
         ) VALUES (?,?,?,?,?,?,?,?,?,CAST(? AS JSON),?,?,?,?,?,?,
                   ?,?,?,?,?, CAST(? AS VARCHAR[]),?,?)",
        params![
            id,
            new.experiment_name,
            new.exercise_type.as_str(),
            new.executed_at_ns,
            new.hypothesis,
            new.method,
            new.outcome.as_str(),
            new.hypothesis_met,
            new.findings,
            serde_json::to_string(&new.action_items).unwrap_or_else(|_| "[]".into()),
            new.target_system,
            new.target_environment,
            new.blast_radius,
            new.recovery_time_s,
            new.duration_s,
            "manual",
            new.entered_by,
            now_ns(),
            new.attestation,
            "draft",
            new.renewal_due_ns,
            serde_json::to_string(&new.framework_refs).unwrap_or_else(|_| "[]".into()),
            batch_id,
            hash,
        ],
    )?;
    Ok((id, hash))
}

/// Append one audit row (no transaction; caller wraps).
fn append_audit(
    conn: &Connection,
    experiment_id: &str,
    changed_by: &str,
    action: &str,
    diff: &serde_json::Value,
    prev_hash: Option<&str>,
    new_hash: &str,
) -> Result<(), ManualError> {
    conn.execute(
        "INSERT INTO manual_experiment_audit (
            id, experiment_id, changed_by, changed_at_ns, action, diff, prev_hash, new_hash
         ) VALUES (?,?,?,?,?,CAST(? AS JSON),?,?)",
        params![
            ulid(),
            experiment_id,
            changed_by,
            now_ns(),
            action,
            diff.to_string(),
            prev_hash,
            new_hash,
        ],
    )?;
    Ok(())
}

/// Recompute the content hash after a lifecycle/status change by reading
/// the row back and re-serializing the canonical fields.
fn rehash(conn: &Connection, id: &str) -> Result<String, ManualError> {
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

impl Writer {
    /// Create a manual test record as a `draft`. Returns the new id.
    ///
    /// # Errors
    /// Returns `Invalid` when required fields are empty, `Store` on write failure.
    pub fn create_manual_draft(&self, new: &NewManualExperiment) -> Result<String, ManualError> {
        new.validate()?;
        with_tx(&self.conn, || {
            let (id, hash) = insert_draft(&self.conn, new, None)?;
            append_audit(
                &self.conn,
                &id,
                &new.entered_by,
                "create",
                &serde_json::json!({
                    "experiment_name": new.experiment_name,
                    "exercise_type": new.exercise_type.as_str(),
                    "outcome_status": new.outcome.as_str(),
                }),
                None,
                &hash,
            )?;
            Ok(id)
        })
    }

    /// Replace the content of a draft (PUT semantics). `entered_by` and
    /// `entered_at_ns` are preserved from the original record.
    ///
    /// # Errors
    /// Returns `NotFound`, `WrongStatus` when the record is no longer a
    /// draft, or `Invalid` for empty required fields.
    pub fn update_manual_draft(
        &self,
        id: &str,
        new: &NewManualExperiment,
        changed_by: &str,
    ) -> Result<(), ManualError> {
        new.validate()?;
        with_tx(&self.conn, || {
            let row =
                fetch_row(&self.conn, id)?.ok_or_else(|| ManualError::NotFound(id.to_string()))?;
            let status = status_of(&row);
            if status != "draft" {
                return Err(ManualError::WrongStatus {
                    status,
                    action: "edit".into(),
                });
            }
            let prev_hash = row
                .get("content_hash")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let entered_by = row
                .get("entered_by")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            self.conn.execute(
                "UPDATE manual_experiments SET
                    experiment_name = ?, exercise_type = ?, executed_at_ns = ?,
                    hypothesis = ?, method = ?, outcome_status = ?,
                    hypothesis_met = ?, findings = ?, action_items = CAST(? AS JSON),
                    target_system = ?, target_environment = ?, blast_radius = ?,
                    recovery_time_s = ?, duration_s = ?, attestation = ?,
                    renewal_due_ns = ?, framework_refs = CAST(? AS VARCHAR[])
                 WHERE id = ?",
                params![
                    new.experiment_name,
                    new.exercise_type.as_str(),
                    new.executed_at_ns,
                    new.hypothesis,
                    new.method,
                    new.outcome.as_str(),
                    new.hypothesis_met,
                    new.findings,
                    serde_json::to_string(&new.action_items).unwrap_or_else(|_| "[]".into()),
                    new.target_system,
                    new.target_environment,
                    new.blast_radius,
                    new.recovery_time_s,
                    new.duration_s,
                    new.attestation,
                    new.renewal_due_ns,
                    serde_json::to_string(&new.framework_refs).unwrap_or_else(|_| "[]".into()),
                    id,
                ],
            )?;
            let hash = rehash(&self.conn, id)?;
            self.conn.execute(
                "UPDATE manual_experiments SET content_hash = ? WHERE id = ?",
                params![hash, id],
            )?;
            append_audit(
                &self.conn,
                id,
                changed_by,
                "edit",
                &serde_json::json!({
                    "replaced_by": entered_by,
                    "fields": [
                        "experiment_name","exercise_type","executed_at_ns","hypothesis",
                        "method","outcome_status","hypothesis_met","findings",
                        "action_items","target_system","target_environment","blast_radius",
                        "recovery_time_s","duration_s","attestation","renewal_due_ns",
                        "framework_refs"
                    ],
                }),
                Some(&prev_hash),
                &hash,
            )?;
            Ok(())
        })
    }

    /// Move a draft to `submitted`, locking it. A fresh attestation text may
    /// be supplied; otherwise the existing one must be non-empty.
    ///
    /// # Errors
    /// Returns `NotFound`, `WrongStatus`, or `Invalid` when no attestation
    /// text is present.
    pub fn submit_manual(
        &self,
        id: &str,
        attestation: Option<&str>,
        by: &str,
    ) -> Result<(), ManualError> {
        with_tx(&self.conn, || {
            let row =
                fetch_row(&self.conn, id)?.ok_or_else(|| ManualError::NotFound(id.to_string()))?;
            let status = status_of(&row);
            if status != "draft" {
                return Err(ManualError::WrongStatus {
                    status,
                    action: "submit".into(),
                });
            }
            let prev_hash = row
                .get("content_hash")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let effective = attestation
                .map(str::to_owned)
                .or_else(|| {
                    row.get("attestation")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_default();
            if effective.trim().is_empty() {
                return Err(ManualError::Invalid(
                    "submit requires a non-empty attestation".into(),
                ));
            }
            self.conn.execute(
                "UPDATE manual_experiments SET status = 'submitted', attestation = ? \
                 WHERE id = ?",
                params![effective, id],
            )?;
            let hash = rehash(&self.conn, id)?;
            self.conn.execute(
                "UPDATE manual_experiments SET content_hash = ? WHERE id = ?",
                params![hash, id],
            )?;
            append_audit(
                &self.conn,
                id,
                by,
                "submit",
                &serde_json::json!({"from_status": "draft", "to_status": "submitted"}),
                Some(&prev_hash),
                &hash,
            )?;
            Ok(())
        })
    }

    /// Verify a submitted record. The reviewer must differ from the person
    /// who entered the record (segregation of duties).
    ///
    /// # Errors
    /// Returns `NotFound`, `WrongStatus` (not submitted), or `SelfReview`.
    pub fn verify_manual(
        &self,
        id: &str,
        reviewer: &str,
        note: Option<&str>,
    ) -> Result<(), ManualError> {
        self.review(id, reviewer, note, true)
    }

    /// Reject a submitted record (a review note is mandatory). The reviewer
    /// must differ from the person who entered the record.
    ///
    /// # Errors
    /// Returns `NotFound`, `WrongStatus`, `SelfReview`, or `Invalid` when the
    /// note is empty.
    pub fn reject_manual(&self, id: &str, reviewer: &str, note: &str) -> Result<(), ManualError> {
        if note.trim().is_empty() {
            return Err(ManualError::Invalid(
                "reject requires a non-empty review note".into(),
            ));
        }
        self.review(id, reviewer, Some(note), false)
    }

    fn review(
        &self,
        id: &str,
        reviewer: &str,
        note: Option<&str>,
        approve: bool,
    ) -> Result<(), ManualError> {
        let action = if approve { "verify" } else { "reject" };
        let to_status = if approve { "verified" } else { "rejected" };
        with_tx(&self.conn, || {
            let row =
                fetch_row(&self.conn, id)?.ok_or_else(|| ManualError::NotFound(id.to_string()))?;
            let status = status_of(&row);
            if status != "submitted" {
                return Err(ManualError::WrongStatus {
                    status,
                    action: action.into(),
                });
            }
            let entered_by = row
                .get("entered_by")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if reviewer.trim().is_empty() {
                return Err(ManualError::Invalid("reviewer must not be empty".into()));
            }
            if reviewer == entered_by {
                return Err(ManualError::SelfReview);
            }
            let prev_hash = row
                .get("content_hash")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            self.conn.execute(
                "UPDATE manual_experiments SET status = ?, reviewed_by = ?, \
                 reviewed_at_ns = ?, review_note = ? WHERE id = ?",
                params![to_status, reviewer, now_ns(), note, id],
            )?;
            let hash = rehash(&self.conn, id)?;
            self.conn.execute(
                "UPDATE manual_experiments SET content_hash = ? WHERE id = ?",
                params![hash, id],
            )?;
            append_audit(
                &self.conn,
                id,
                reviewer,
                action,
                &serde_json::json!({
                    "from_status": "submitted",
                    "to_status": to_status,
                    "note": note,
                }),
                Some(&prev_hash),
                &hash,
            )?;
            Ok(())
        })
    }

    /// Attach an external evidence link to a draft or submitted record.
    /// Returns the attachment id.
    ///
    /// # Errors
    /// Returns `NotFound`, `WrongStatus` (verified/rejected records are
    /// locked), or `Invalid` for an empty URI.
    pub fn add_manual_attachment(
        &self,
        experiment_id: &str,
        kind: AttachmentKind,
        uri: &str,
        label: Option<&str>,
        file_hash: Option<&str>,
        added_by: &str,
    ) -> Result<String, ManualError> {
        if uri.trim().is_empty() {
            return Err(ManualError::Invalid(
                "attachment uri must not be empty".into(),
            ));
        }
        with_tx(&self.conn, || {
            let row = fetch_row(&self.conn, experiment_id)?
                .ok_or_else(|| ManualError::NotFound(experiment_id.to_string()))?;
            let status = status_of(&row);
            if status != "draft" && status != "submitted" {
                return Err(ManualError::WrongStatus {
                    status,
                    action: "attach".into(),
                });
            }
            let content_hash = row
                .get("content_hash")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            let attachment_id = ulid();
            self.conn.execute(
                "INSERT INTO evidence_attachments (
                    id, experiment_id, kind, uri, label, file_hash, added_by, added_at_ns
                 ) VALUES (?,?,?,?,?,?,?,?)",
                params![
                    attachment_id,
                    experiment_id,
                    kind.as_str(),
                    uri,
                    label,
                    file_hash,
                    added_by,
                    now_ns(),
                ],
            )?;
            // Attachments do not alter the content hash; the audit row still
            // chains prev == new so the sequence stays verifiable.
            append_audit(
                &self.conn,
                experiment_id,
                added_by,
                "attach",
                &serde_json::json!({
                    "attachment_id": attachment_id,
                    "kind": kind.as_str(),
                    "uri": uri,
                    "label": label,
                }),
                Some(&content_hash),
                &content_hash,
            )?;
            Ok(attachment_id)
        })
    }

    /// Bulk-import records as drafts in one transaction, recording an
    /// `import_batches` row. Attestation is NOT bypassed: every item carries
    /// its own attestation text and still needs the submit → verify path to
    /// score. Returns `(batch_id, experiment_ids)`.
    ///
    /// # Errors
    /// Returns `Invalid` for any item failing validation (the whole batch
    /// rolls back), or `Store` on write failure.
    pub fn import_manual_drafts(
        &self,
        items: &[NewManualExperiment],
        label: Option<String>,
    ) -> Result<(String, Vec<String>), ManualError> {
        if items.is_empty() {
            return Err(ManualError::Invalid(
                "import requires at least one record".into(),
            ));
        }
        for item in items {
            item.validate()?;
        }
        with_tx(&self.conn, || {
            let batch_id = format!("manual-import-{}", ulid());
            self.conn.execute(
                "INSERT INTO import_batches (id, source, imported_at_ns, rows, label) \
                 VALUES (?,?,?,?,?)",
                params![
                    batch_id,
                    "manual-api",
                    now_ns(),
                    i32::try_from(items.len()).unwrap_or(i32::MAX),
                    label,
                ],
            )?;
            let mut ids = Vec::with_capacity(items.len());
            for item in items {
                let (id, hash) = insert_draft(&self.conn, item, Some(&batch_id))?;
                append_audit(
                    &self.conn,
                    &id,
                    &item.entered_by,
                    "create",
                    &serde_json::json!({
                        "experiment_name": item.experiment_name,
                        "batch_id": batch_id,
                        "via": "import",
                    }),
                    None,
                    &hash,
                )?;
                ids.push(id);
            }
            Ok((batch_id, ids))
        })
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Store;

    const DAY: i64 = 86_400 * 1_000_000_000;

    fn temp_writer() -> (tempfile::TempDir, crate::Store, Writer) {
        let d = tempfile::TempDir::new().unwrap();
        let store = Store::open(&d.path().join("kronika.duckdb")).unwrap();
        let writer = store.writer().unwrap();
        (d, store, writer)
    }

    fn draft(by: &str) -> NewManualExperiment {
        NewManualExperiment {
            experiment_name: "edge-cache-outage".into(),
            exercise_type: ExerciseType::GameDay,
            executed_at_ns: 100 * DAY,
            hypothesis: "Static asset failover keeps p95 under 800ms".into(),
            method: "Disabled the primary CDN PoP; observed failover".into(),
            outcome: ManualOutcome::Passed,
            hypothesis_met: Some(true),
            findings: Some("Failover worked; warm-up took 40s".into()),
            action_items: vec!["Pre-warm the secondary PoP".into()],
            target_system: Some("cdn".into()),
            target_environment: Some("production".into()),
            blast_radius: Some("single-pop".into()),
            recovery_time_s: Some(40.0),
            duration_s: Some(3600.0),
            entered_by: by.into(),
            attestation: "I attest this record reflects the exercise as executed.".into(),
            renewal_due_ns: Some(190 * DAY),
            framework_refs: vec!["DORA Art. 24(7)".into()],
        }
    }

    #[test]
    fn create_submit_verify_happy_path() {
        let (_d, store, writer) = temp_writer();
        let id = writer.create_manual_draft(&draft("alice")).unwrap();
        assert_eq!(id.len(), 26);

        writer.submit_manual(&id, None, "alice").unwrap();
        writer
            .verify_manual(&id, "bob", Some("evidence reviewed"))
            .unwrap();

        let reader = store.read_only().unwrap();
        let detail = reader.manual_experiment_detail(&id).unwrap().unwrap();
        assert_eq!(detail.experiment["status"], serde_json::json!("verified"));
        assert_eq!(detail.experiment["reviewed_by"], serde_json::json!("bob"));
        assert_eq!(
            detail.experiment["framework_refs"],
            serde_json::json!(["DORA Art. 24(7)"])
        );
        // Audit: create + submit + verify, hash chain intact.
        let actions: Vec<&str> = detail
            .audit
            .iter()
            .map(|a| a["action"].as_str().unwrap())
            .collect();
        assert_eq!(actions, ["create", "submit", "verify"]);
        assert!(detail.audit[0]["prev_hash"].is_null());
        for w in detail.audit.windows(2) {
            assert_eq!(w[0]["new_hash"], w[1]["prev_hash"]);
        }
        let last = detail.audit.last().unwrap()["new_hash"].clone();
        assert_eq!(last, detail.experiment["content_hash"]);
    }

    #[test]
    fn draft_edit_then_submit_locks() {
        let (_d, _store, writer) = temp_writer();
        let id = writer.create_manual_draft(&draft("alice")).unwrap();

        let mut edited = draft("alice");
        edited.findings = Some("updated findings".into());
        writer.update_manual_draft(&id, &edited, "alice").unwrap();

        writer.submit_manual(&id, None, "alice").unwrap();
        // Edits after submit are rejected — the record is locked.
        let err = writer
            .update_manual_draft(&id, &edited, "alice")
            .unwrap_err();
        assert!(matches!(err, ManualError::WrongStatus { .. }));
    }

    #[test]
    fn self_review_is_rejected() {
        let (_d, _store, writer) = temp_writer();
        let id = writer.create_manual_draft(&draft("alice")).unwrap();
        writer.submit_manual(&id, None, "alice").unwrap();
        let err = writer.verify_manual(&id, "alice", None).unwrap_err();
        assert!(matches!(err, ManualError::SelfReview));
        let err = writer.reject_manual(&id, "alice", "no").unwrap_err();
        assert!(matches!(err, ManualError::SelfReview));
    }

    #[test]
    fn reject_requires_note_and_wrong_status_is_conflict() {
        let (_d, _store, writer) = temp_writer();
        let id = writer.create_manual_draft(&draft("alice")).unwrap();
        // Verify requires submitted first.
        let err = writer.verify_manual(&id, "bob", None).unwrap_err();
        assert!(matches!(err, ManualError::WrongStatus { .. }));
        writer.submit_manual(&id, None, "alice").unwrap();
        let err = writer.reject_manual(&id, "bob", "  ").unwrap_err();
        assert!(matches!(err, ManualError::Invalid(_)));
        writer
            .reject_manual(&id, "bob", "insufficient evidence")
            .unwrap();
    }

    #[test]
    fn attachments_chain_audit_without_changing_hash() {
        let (_d, store, writer) = temp_writer();
        let id = writer.create_manual_draft(&draft("alice")).unwrap();
        let attachment = writer
            .add_manual_attachment(
                &id,
                AttachmentKind::Url,
                "https://wiki.example.com/gameday-2026-07",
                Some("write-up"),
                None,
                "alice",
            )
            .unwrap();
        writer.submit_manual(&id, None, "alice").unwrap();
        writer.verify_manual(&id, "bob", None).unwrap();
        // Verified records are locked for attachments too.
        let err = writer
            .add_manual_attachment(&id, AttachmentKind::Url, "https://x", None, None, "bob")
            .unwrap_err();
        assert!(matches!(err, ManualError::WrongStatus { .. }));

        let reader = store.read_only().unwrap();
        let detail = reader.manual_experiment_detail(&id).unwrap().unwrap();
        assert_eq!(detail.attachments.len(), 1);
        assert_eq!(detail.attachments[0]["id"], serde_json::json!(attachment));
        // Attach audit row chains prev == new (content unchanged).
        let attach = detail
            .audit
            .iter()
            .find(|a| a["action"] == serde_json::json!("attach"))
            .unwrap();
        assert_eq!(attach["prev_hash"], attach["new_hash"]);
    }

    #[test]
    fn bulk_import_lands_as_drafts_with_batch() {
        let (_d, store, writer) = temp_writer();
        let items = vec![draft("alice"), {
            let mut d = draft("carol");
            d.experiment_name = "db-failover".into();
            d.exercise_type = ExerciseType::Failover;
            d.outcome = ManualOutcome::Partial;
            d
        }];
        let (batch_id, ids) = writer
            .import_manual_drafts(&items, Some("q3-backfill".into()))
            .unwrap();
        assert_eq!(ids.len(), 2);

        let reader = store.read_only().unwrap();
        let rows = reader.manual_experiments(Some("draft")).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows
            .iter()
            .all(|r| r["batch_id"] == serde_json::json!(batch_id)));
        let batches = reader
            .query_json_rows("SELECT source, rows, label FROM import_batches")
            .unwrap();
        assert_eq!(batches[0]["source"], serde_json::json!("manual-api"));
        assert_eq!(batches[0]["rows"], serde_json::json!(2));
    }

    #[test]
    fn import_validates_every_item_and_rolls_back() {
        let (_d, store, writer) = temp_writer();
        let mut bad = draft("alice");
        bad.hypothesis = String::new();
        let err = writer
            .import_manual_drafts(&[draft("bob"), bad], None)
            .unwrap_err();
        assert!(matches!(err, ManualError::Invalid(_)));
        let reader = store.read_only().unwrap();
        assert_eq!(reader.manual_experiments(None).unwrap().len(), 0);
    }

    #[test]
    fn missing_record_is_not_found() {
        let (_d, _store, writer) = temp_writer();
        let err = writer.submit_manual("01JNONE", None, "alice").unwrap_err();
        assert!(matches!(err, ManualError::NotFound(_)));
    }
}
