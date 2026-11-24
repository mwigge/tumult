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
//!
//! A plan (schema v22, [`plans`]) says which exercise is due when. It can
//! be edited while it is planned; the verified record that names it
//! fulfils it, and the record stays as it was signed off.
//!
//! Verifying a record is a sign-off (schema v22): a record stays
//! `submitted` until it has the approvals its exercise type needs, each
//! from another user, and a reject by any reviewer rejects it. Each
//! sign-off is a `manual_signoffs` row chained to the one before it, and
//! every audit row links the row before it ([`chain`]), so the audit
//! verify ([`verify`]) finds a direct edit or a deleted row.
//!
//! A file may be attached as evidence in any status ([`files`]): it is
//! stored once in the lake folder by its SHA-256, and its attach audit
//! row names it without changing the seal.
//!
//! A record cites only DORA requirements the registry knows (its
//! `framework_refs` are checked on every write), and names only plan
//! scenarios of RTS 2024/1774 (its `scenarios`, schema v26). Its findings and actions
//! have a [`Severity`], an action may name its [`Retest`], a record names
//! its [`Tester`], and a record of type `tlpt` holds its [`TlptDetail`].
//! All of them are sealed.

mod chain;
pub(crate) mod files;
mod hash;
mod plans;
mod reader;
mod verify;
mod writer;

use std::fmt;

use tumult_core::compliance::{
    check_reference, check_scenario, UnknownRequirement, UnknownScenario,
};
pub use tumult_core::types::{Tester, TesterKind};

use crate::StoreError;

pub use files::{
    attachments_dir, stored_file, FileError, FileUpload, StagedFile, MAX_FILE_BYTES, MEDIA_TYPES,
};
pub use plans::{NewExercisePlan, PlanDetail, PlanEdit, PlanFilter, PlanStatus};
pub use reader::ManualDetail;
pub use verify::{AuditProblem, AuditVerdict};

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

/// Evidence attachment kinds. A `url` or a `ticket` is a link; a `file` is
/// stored in the lake folder (the `files` module). `log_excerpt` is reserved.
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

/// How much a finding or an action matters. DORA Art. 24(5) asks that the
/// issues a test reveals are prioritised and classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Critical => "critical",
        }
    }

    pub fn parse(s: &str) -> Result<Self, ManualError> {
        Ok(match s {
            "low" => Self::Low,
            "medium" => Self::Medium,
            "high" => Self::High,
            "critical" => Self::Critical,
            other => {
                return Err(ManualError::Invalid(format!(
                    "unknown severity '{other}' (low|medium|high|critical)"
                )))
            }
        })
    }
}

/// What retested an action once it was done: an automated run, by its
/// experiment id, or another manual record, by its id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetestKind {
    Run,
    Record,
}

impl RetestKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Record => "record",
        }
    }

    pub fn parse(s: &str) -> Result<Self, ManualError> {
        Ok(match s {
            "run" => Self::Run,
            "record" => Self::Record,
            other => {
                return Err(ManualError::Invalid(format!(
                    "unknown retest kind '{other}' (run|record)"
                )))
            }
        })
    }
}

/// The run or the record that retested an action. Only the kind is
/// checked, as for a link: the retest may be stored later or elsewhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retest {
    pub kind: RetestKind,
    pub id: String,
}

/// The TLPT facts of a record of type `tlpt`, under the TLPT RTS
/// (Delegated Regulation (EU) 2025/1190): the attestation the TLPT
/// authority issued and its date (Art. 14), and the test summary report
/// (Art. 12). The remediation plan (Art. 13) is the actions of the record,
/// each with its owner, due date, severity and retest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlptDetail {
    /// The reference of the attestation, as the TLPT authority gave it.
    pub attestation_ref: Option<String>,
    /// When the attestation was issued, in unix nanoseconds.
    pub attested_at_ns: Option<i64>,
    /// The reference of the test summary report.
    pub summary_report_ref: Option<String>,
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
    pub severity: Option<Severity>,
    /// The run or the record that retested the action.
    pub retest: Option<Retest>,
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
    /// The plan this exercise fulfils once the record is verified. It must
    /// name a planned plan of the same exercise type.
    pub plan_id: Option<String>,
    /// How much the findings matter.
    pub findings_severity: Option<Severity>,
    /// Who ran the test, internal or external (DORA Art. 24(4)).
    pub tester: Option<Tester>,
    /// The TLPT facts. Only a record of type `tlpt` takes them.
    pub tlpt: Option<TlptDetail>,
    /// The plan scenarios of RTS 2024/1774 the exercise tested, by id
    /// (`premises_loss`, `power_outage`, …), each one of the closed set
    /// (schema v26).
    pub scenarios: Vec<String>,
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
            if let Some(retest) = &action.retest {
                require("action", n, "retest id", &retest.id)?;
            }
        }
        if self
            .tester
            .as_ref()
            .is_some_and(|tester| tester.name.trim().is_empty())
        {
            return Err(ManualError::Invalid(
                "'tester.name' must not be empty".into(),
            ));
        }
        if let Some(tlpt) = &self.tlpt {
            tlpt.validate()?;
        }
        for scenario in &self.scenarios {
            check_scenario(scenario).map_err(ManualError::UnknownScenario)?;
        }
        for (n, link) in (1..).zip(&self.links) {
            require("link", n, "linked_id", &link.linked_id)?;
        }
        if self
            .plan_id
            .as_deref()
            .is_some_and(|id| id.trim().is_empty())
        {
            return Err(ManualError::Invalid("'plan_id' must not be empty".into()));
        }
        Ok(())
    }
}

impl TlptDetail {
    fn validate(&self) -> Result<(), ManualError> {
        for (field, value) in [
            ("attestation_ref", &self.attestation_ref),
            ("summary_report_ref", &self.summary_report_ref),
        ] {
            if value.as_deref().is_some_and(|v| v.trim().is_empty()) {
                return Err(ManualError::Invalid(format!(
                    "tlpt '{field}' must not be empty"
                )));
            }
        }
        if self.attested_at_ns.is_some_and(|at| at <= 0) {
            return Err(ManualError::Invalid(
                "tlpt 'attested_at_ns' must be a positive unix-nanos timestamp".into(),
            ));
        }
        Ok(())
    }
}

/// How a reviewer decides on a submitted record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignOffDecision {
    Approved,
    Rejected,
}

impl SignOffDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
        }
    }
}

/// One sign-off of a submitted record. A reject needs a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignOff<'a> {
    pub by: &'a str,
    pub note: Option<&'a str>,
    pub decision: SignOffDecision,
}

/// Where a record stands after a sign-off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignOffOutcome {
    /// `submitted` while approvals are missing, else `verified` or
    /// `rejected`.
    pub status: String,
    /// The approving sign-offs the record has now.
    pub approvals: u32,
    /// The approvals its exercise type needs.
    pub required: u32,
}

/// Errors about a plan. The API layer maps: `Invalid` → 400, `NotFound` →
/// 404, `WrongStatus` → 409, `Unusable` → 422.
#[derive(Debug)]
pub enum PlanError {
    /// Bad input (empty required field, interval out of range).
    Invalid(String),
    /// No plan with this id.
    NotFound(String),
    /// The plan is no longer planned, so it cannot take this action.
    WrongStatus { status: String, action: String },
    /// A record names a plan it cannot fulfil: the message says why.
    Unusable(String),
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(msg) => write!(f, "invalid plan: {msg}"),
            Self::NotFound(id) => write!(f, "plan '{id}' not found"),
            Self::WrongStatus { status, action } => {
                write!(f, "cannot {action} a plan in status '{status}'")
            }
            Self::Unusable(msg) => write!(f, "{msg}"),
        }
    }
}

/// Errors from the manual-evidence lifecycle. The API layer maps:
/// `Invalid` → 400, `NotFound` → 404, `WrongStatus` → 409,
/// `SelfReview`/`AlreadySigned`/`UnknownRequirement`/`UnknownScenario` →
/// 422, `Plan` as [`PlanError`] says, `Store` → 500.
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
    /// This user has signed the record off already: one user signs once.
    AlreadySigned(String),
    /// A framework reference names a DORA requirement the registry does
    /// not know.
    UnknownRequirement(UnknownRequirement),
    /// A scenario is not a plan scenario of RTS 2024/1774.
    UnknownScenario(UnknownScenario),
    /// A plan write that cannot be done, or a plan a record cannot fulfil.
    Plan(PlanError),
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
            Self::AlreadySigned(by) => write!(f, "'{by}' has already signed off this record"),
            Self::UnknownRequirement(e) => write!(f, "framework_refs: {e}"),
            Self::UnknownScenario(e) => write!(f, "scenarios: {e}"),
            Self::Plan(e) => write!(f, "{e}"),
            Self::Store(e) => write!(f, "store error: {e}"),
        }
    }
}

impl std::error::Error for ManualError {}

impl From<PlanError> for ManualError {
    fn from(e: PlanError) -> Self {
        Self::Plan(e)
    }
}

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
    /// The requirements the record cites. A DORA requirement must be one
    /// the registry knows: its registry id (`dora.art24.6`) or an older
    /// form of it (`DORA Art. 24(6)`), kept as written. A reference to
    /// another framework (`ISO 27001 A.5.30`) is kept unchecked.
    pub framework_refs: Vec<String>,
    /// The DR facts, participants, steps, actions, links and the plan the
    /// record fulfils (schema v22).
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
        for reference in &self.framework_refs {
            check_reference(reference).map_err(ManualError::UnknownRequirement)?;
        }
        if self.exercise.tlpt.is_some() && self.exercise_type != ExerciseType::Tlpt {
            return Err(ManualError::Invalid(
                "the tlpt block is for a record of exercise_type 'tlpt' only".into(),
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

    fn step(started_at_ns: i64, ended_at_ns: i64) -> ExerciseStep {
        ExerciseStep {
            title: "Promote the standby".into(),
            actor: None,
            started_at_ns: Some(started_at_ns),
            ended_at_ns: Some(ended_at_ns),
            result: None,
            note: None,
        }
    }

    #[test]
    fn a_step_may_end_when_it_starts_and_a_dr_number_may_be_zero() {
        // A step that took no time, and an RTO and RPO of 0 s (no outage,
        // no data lost), are what was seen: they are kept.
        let mut at_the_bounds = valid();
        at_the_bounds.exercise.steps = vec![step(5, 5)];
        at_the_bounds.exercise.rto_target_s = Some(0.0);
        at_the_bounds.exercise.rpo_target_s = Some(0.0);
        at_the_bounds.exercise.rpo_measured_s = Some(0.0);
        assert!(
            at_the_bounds.validate().is_ok(),
            "{:?}",
            at_the_bounds.validate()
        );

        let mut backwards = valid();
        backwards.exercise.steps = vec![step(5, 4)];
        let err = backwards.validate().unwrap_err();
        assert!(
            matches!(&err, ManualError::Invalid(msg) if msg == "step 1 ends before it starts"),
            "{err:?}"
        );
        for bad in [-0.001, f64::NAN, f64::INFINITY] {
            let mut below = valid();
            below.exercise.rpo_measured_s = Some(bad);
            let err = below.validate().unwrap_err();
            assert!(
                matches!(&err, ManualError::Invalid(msg) if msg.contains("'rpo_measured_s'")),
                "{bad}: {err:?}"
            );
        }
    }
}
