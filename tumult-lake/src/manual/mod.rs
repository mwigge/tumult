//! Manual evidence: hand-entered test records with attestation and review.
//!
//! Complements the automated (OTLP-ingested) experiment telemetry with
//! records of manually executed resilience tests — game days, tabletop
//! exercises, failover drills, pentests — that never touch an agent.
//!
//! Lifecycle: `draft` (fully mutable) → `submitted` (locked; requires a
//! non-empty attestation) → `verified` / `rejected` by a reviewer who must
//! NOT have created, edited, submitted or attached evidence to the record.
//! This separates contributor and reviewer identities; organizational
//! independence requires external verification. Every mutation appends an
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
    /// A threat-led penetration test under DORA Art. 26, as distinct
    /// from an ordinary penetration test.
    Tlpt,
    Drill,
    /// A restore from a backup, tested end to end.
    Restore,
    /// A backup taken and checked that it can be read back.
    Backup,
    Other,
}

impl ExerciseType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::GameDay => "gameday",
            Self::Tabletop => "tabletop",
            Self::Failover => "failover",
            Self::Pentest => "pentest",
            Self::Tlpt => "tlpt",
            Self::Drill => "drill",
            Self::Restore => "restore",
            Self::Backup => "backup",
            Self::Other => "other",
        }
    }

    pub fn parse(s: &str) -> Result<Self, ManualError> {
        Ok(match s {
            "gameday" => Self::GameDay,
            "tabletop" => Self::Tabletop,
            "failover" => Self::Failover,
            "pentest" => Self::Pentest,
            "tlpt" => Self::Tlpt,
            "drill" => Self::Drill,
            "restore" => Self::Restore,
            "backup" => Self::Backup,
            "other" => Self::Other,
            other => {
                return Err(ManualError::Invalid(format!(
                    "unknown exercise_type '{other}' \
                     (gameday|tabletop|failover|pentest|tlpt|drill|restore|backup|other)"
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

/// The result of one step of an exercise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepResult {
    Done,
    Failed,
    Skipped,
}

impl StepResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    pub fn parse(s: &str) -> Result<Self, ManualError> {
        Ok(match s {
            "done" => Self::Done,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            other => {
                return Err(ManualError::Invalid(format!(
                    "unknown step result '{other}' (done|failed|skipped)"
                )))
            }
        })
    }
}

/// What a link of a record points at. Only the kind is checked: the thing
/// itself may be registered or run later, and may live in another lake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkKind {
    /// A function of the register, by `ict_functions.function_id`.
    Function,
    /// A telemetry entity, by its `ict.entity.id`.
    Entity,
    /// A registered definition, by `run_registry.id`.
    Registry,
    /// An automated run, by its experiment id.
    Run,
    /// A GameDay campaign, by its daemon run id.
    GameDay,
}

impl LinkKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Function => "function",
            Self::Entity => "entity",
            Self::Registry => "registry",
            Self::Run => "run",
            Self::GameDay => "gameday",
        }
    }

    pub fn parse(s: &str) -> Result<Self, ManualError> {
        Ok(match s {
            "function" => Self::Function,
            "entity" => Self::Entity,
            "registry" => Self::Registry,
            "run" => Self::Run,
            "gameday" => Self::GameDay,
            other => {
                return Err(ManualError::Invalid(format!(
                    "unknown link kind '{other}' (function|entity|registry|run|gameday)"
                )))
            }
        })
    }
}

/// One person who took part in an exercise, and the role they had.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Participant {
    pub name: String,
    pub role: Option<String>,
}

/// One step of the timeline of an exercise, in the order it was taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExerciseStep {
    pub title: String,
    pub actor: Option<String>,
    pub started_at_ns: Option<i64>,
    pub ended_at_ns: Option<i64>,
    pub result: Option<StepResult>,
    pub note: Option<String>,
}

/// An action that came out of an exercise, with the person who owns it.
/// It is stored `open`; closing it later does not change the sealed record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionItem {
    pub title: String,
    pub owner: String,
    pub due_at_ns: Option<i64>,
    /// A ticket key or URL that tracks the action.
    pub ticket: Option<String>,
}

/// A link from a record to what the exercise tested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExerciseLink {
    pub kind: LinkKind,
    pub linked_id: String,
    pub label: Option<String>,
}

/// The DR facts of an exercise (schema v22). The actual RTO is
/// [`NewManualExperiment::recovery_time_s`]. Every field is optional, so a
/// record without them is the record of 3.0.3.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExerciseDetail {
    /// The planned RTO, in seconds.
    pub rto_target_s: Option<f64>,
    /// The planned RPO: the seconds of committed writes from before the
    /// outage that may be lost, as the automated RPO metric means it.
    pub rpo_target_s: Option<f64>,
    /// The actual RPO: the seconds of committed writes from before the
    /// outage that were lost.
    pub rpo_measured_s: Option<f64>,
    /// Whether the data was checked after the recovery: `true` complete,
    /// `false` loss or corruption found, `None` not checked.
    pub data_verified: Option<bool>,
    pub participants: Vec<Participant>,
    pub steps: Vec<ExerciseStep>,
    /// The actions with owners. From schema v22 on these are the actions of
    /// a record; [`NewManualExperiment::action_items`] is the older list of
    /// plain texts, kept for the clients that still send it.
    pub actions: Vec<ActionItem>,
    pub links: Vec<ExerciseLink>,
}

/// `Invalid` naming `field` of the `n`th (1-based) `what` when `value` is blank.
fn require(what: &str, n: usize, field: &str, value: &str) -> Result<(), ManualError> {
    if value.trim().is_empty() {
        return Err(ManualError::Invalid(format!(
            "{what} {n}: '{field}' must not be empty"
        )));
    }
    Ok(())
}

impl ExerciseDetail {
    fn validate(&self) -> Result<(), ManualError> {
        for (field, value) in [
            ("rto_target_s", self.rto_target_s),
            ("rpo_target_s", self.rpo_target_s),
            ("rpo_measured_s", self.rpo_measured_s),
        ] {
            if value.is_some_and(|v| !(v.is_finite() && v >= 0.0)) {
                return Err(ManualError::Invalid(format!(
                    "'{field}' must be a number of seconds at or above 0"
                )));
            }
        }
        for (n, p) in (1..).zip(&self.participants) {
            require("participant", n, "name", &p.name)?;
        }
        for (n, step) in (1..).zip(&self.steps) {
            require("step", n, "title", &step.title)?;
            if let (Some(start), Some(end)) = (step.started_at_ns, step.ended_at_ns) {
                if end < start {
                    return Err(ManualError::Invalid(format!(
                        "step {n} ends before it starts"
                    )));
                }
            }
        }
        for (n, action) in (1..).zip(&self.actions) {
            require("action", n, "title", &action.title)?;
            require("action", n, "owner", &action.owner)?;
        }
        for (n, link) in (1..).zip(&self.links) {
            require("link", n, "linked_id", &link.linked_id)?;
        }
        Ok(())
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
    /// Reviewer must not have contributed to the record.
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
                    "reviewer must not have created, edited, submitted, or attached evidence to the record"
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
    /// The DR facts, participants, steps, actions and links (schema v22).
    pub exercise: ExerciseDetail,
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
        self.exercise.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exercise_type_parses_every_kind_and_rejects_unknown() {
        for (s, want) in [
            ("gameday", ExerciseType::GameDay),
            ("tabletop", ExerciseType::Tabletop),
            ("failover", ExerciseType::Failover),
            ("pentest", ExerciseType::Pentest),
            ("tlpt", ExerciseType::Tlpt),
            ("drill", ExerciseType::Drill),
            ("restore", ExerciseType::Restore),
            ("backup", ExerciseType::Backup),
            ("other", ExerciseType::Other),
        ] {
            assert_eq!(ExerciseType::parse(s).unwrap(), want);
            assert_eq!(want.as_str(), s);
        }
        let err = ExerciseType::parse("workshop").unwrap_err();
        assert!(
            matches!(&err, ManualError::Invalid(msg) if msg.contains("unknown exercise_type 'workshop'")),
            "{err:?}"
        );
    }

    #[test]
    fn manual_outcome_parses_every_outcome_and_rejects_unknown() {
        for (s, want) in [
            ("passed", ManualOutcome::Passed),
            ("failed", ManualOutcome::Failed),
            ("partial", ManualOutcome::Partial),
            ("inconclusive", ManualOutcome::Inconclusive),
        ] {
            assert_eq!(ManualOutcome::parse(s).unwrap(), want);
            assert_eq!(want.as_str(), s);
        }
        let err = ManualOutcome::parse("unknown").unwrap_err();
        assert!(
            matches!(&err, ManualError::Invalid(msg) if msg.contains("unknown outcome_status 'unknown'")),
            "{err:?}"
        );
    }

    #[test]
    fn attachment_kind_parses_every_kind_and_rejects_unknown() {
        for (s, want) in [
            ("file", AttachmentKind::File),
            ("url", AttachmentKind::Url),
            ("log_excerpt", AttachmentKind::LogExcerpt),
            ("ticket", AttachmentKind::Ticket),
        ] {
            assert_eq!(AttachmentKind::parse(s).unwrap(), want);
            assert_eq!(want.as_str(), s);
        }
        let err = AttachmentKind::parse("screenshot").unwrap_err();
        assert!(
            matches!(&err, ManualError::Invalid(msg) if msg.contains("unknown attachment kind 'screenshot'")),
            "{err:?}"
        );
    }

    #[test]
    fn step_result_parses_every_result_and_rejects_unknown() {
        for (s, want) in [
            ("done", StepResult::Done),
            ("failed", StepResult::Failed),
            ("skipped", StepResult::Skipped),
        ] {
            assert_eq!(StepResult::parse(s).unwrap(), want);
            assert_eq!(want.as_str(), s);
        }
        let err = StepResult::parse("ok").unwrap_err();
        assert!(
            matches!(&err, ManualError::Invalid(msg) if msg.contains("unknown step result 'ok'")),
            "{err:?}"
        );
    }

    #[test]
    fn link_kind_parses_every_kind_and_rejects_unknown() {
        for (s, want) in [
            ("function", LinkKind::Function),
            ("entity", LinkKind::Entity),
            ("registry", LinkKind::Registry),
            ("run", LinkKind::Run),
            ("gameday", LinkKind::GameDay),
        ] {
            assert_eq!(LinkKind::parse(s).unwrap(), want);
            assert_eq!(want.as_str(), s);
        }
        let err = LinkKind::parse("service").unwrap_err();
        assert!(
            matches!(&err, ManualError::Invalid(msg) if msg.contains("unknown link kind 'service'")),
            "{err:?}"
        );
    }

    #[test]
    fn manual_error_display_names_each_cause() {
        assert_eq!(
            ManualError::Invalid("bad".into()).to_string(),
            "invalid manual experiment: bad"
        );
        assert_eq!(
            ManualError::NotFound("m-1".into()).to_string(),
            "manual experiment 'm-1' not found"
        );
        assert_eq!(
            ManualError::WrongStatus {
                status: "verified".into(),
                action: "edit".into(),
            }
            .to_string(),
            "cannot edit a record in status 'verified'"
        );
        assert_eq!(
            ManualError::SelfReview.to_string(),
            "reviewer must not have created, edited, submitted, or attached evidence to the record"
        );
    }

    fn valid() -> NewManualExperiment {
        NewManualExperiment {
            experiment_name: "exp".into(),
            exercise_type: ExerciseType::Drill,
            executed_at_ns: 1,
            hypothesis: "h".into(),
            method: "m".into(),
            outcome: ManualOutcome::Passed,
            hypothesis_met: None,
            findings: None,
            action_items: vec![],
            target_system: None,
            target_environment: None,
            blast_radius: None,
            recovery_time_s: None,
            duration_s: None,
            entered_by: "alice".into(),
            attestation: "attested".into(),
            renewal_due_ns: None,
            framework_refs: vec![],
            exercise: ExerciseDetail::default(),
        }
    }

    #[test]
    fn validate_rejects_blank_required_fields_and_bad_timestamps() {
        assert!(valid().validate().is_ok());

        let mut blank = valid();
        blank.method = "   ".into();
        let err = blank.validate().unwrap_err();
        assert!(
            matches!(&err, ManualError::Invalid(msg) if msg == "'method' must not be empty"),
            "{err:?}"
        );

        let mut no_attestation = valid();
        no_attestation.attestation = String::new();
        let err = no_attestation.validate().unwrap_err();
        assert!(
            matches!(&err, ManualError::Invalid(msg) if msg == "'attestation' must not be empty"),
            "{err:?}"
        );

        let mut epoch = valid();
        epoch.executed_at_ns = 0;
        let err = epoch.validate().unwrap_err();
        assert!(
            matches!(&err, ManualError::Invalid(msg) if msg.contains("positive unix-nanos")),
            "{err:?}"
        );
    }
}
