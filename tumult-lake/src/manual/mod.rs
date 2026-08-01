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

mod hash;
mod reader;
mod writer;

use std::fmt;

use crate::StoreError;

pub use reader::ManualDetail;

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
