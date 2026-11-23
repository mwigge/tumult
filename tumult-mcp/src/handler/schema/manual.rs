//! Manual registry tool schemas: list and read the records and the plans,
//! and draft a record. No tool submits, signs off, verifies or attaches
//! evidence: an agent may draft, a person signs (segregation of duties).

use rust_mcp_sdk::macros;

use super::{default_list_limit, default_store_path};

/// Arguments for the `tumult_manual_list` tool.
#[macros::mcp_tool(
    name = "tumult_manual_list",
    description = "List the records of the manual registry (DR exercises such as a failover, a restore test or a tabletop, and other manual tests), newest entry first. Filters: status (draft, submitted, verified or rejected), exercise_type, target (the target system), from and to (a date YYYY-MM-DD in UTC or an RFC 3339 time; the record was executed at or after from and before to, and a date as to counts its whole day). Supports limit (default 100, max 1000) and offset; structured content is {items, total, offset, limit}. Reads the lake read-only, so the daemon must not hold it. Segregation of duties: an agent may draft, a person signs. No MCP tool submits, signs off, verifies or attaches evidence; a person does that in the daemon.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualListTool {
    /// Only records in this status: `draft`, `submitted`, `verified` or
    /// `rejected`.
    pub status: Option<String>,
    /// Only records of this exercise type, such as `failover` or `restore`.
    pub exercise_type: Option<String>,
    /// Only records of this target system.
    pub target: Option<String>,
    /// Only records executed at or after this date or time.
    pub from: Option<String>,
    /// Only records executed before this time; a date counts its whole day.
    pub to: Option<String>,
    /// Maximum number of records returned (default 100, max 1000).
    #[serde(default = "default_list_limit")]
    pub limit: u64,
    /// Number of records to skip before the returned page.
    #[serde(default)]
    pub offset: u64,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

/// Arguments for the `tumult_manual_get` tool.
#[macros::mcp_tool(
    name = "tumult_manual_get",
    description = "Read one record of the manual registry with its audit trail, attachments, steps, actions, links and sign-offs. Structured content is {experiment, audit, attachments, steps, actions, links, signoffs}. Reads the lake read-only, so the daemon must not hold it. Segregation of duties: an agent may draft, a person signs. No MCP tool submits, signs off, verifies or attaches evidence; a person does that in the daemon.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualGetTool {
    /// The id of the record (from `tumult_manual_list`).
    pub id: String,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

/// One person who took part in the exercise, and their role.
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualParticipantArg {
    pub name: String,
    pub role: Option<String>,
}

/// One step of the timeline of the exercise.
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualStepArg {
    pub title: String,
    pub actor: Option<String>,
    pub started_at_ns: Option<i64>,
    pub ended_at_ns: Option<i64>,
    /// `done`, `failed` or `skipped`.
    pub result: Option<String>,
    pub note: Option<String>,
}

/// One action that came out of the exercise, with the person who owns it.
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualActionArg {
    pub title: String,
    pub owner: String,
    pub due_at_ns: Option<i64>,
    /// A ticket key or URL that tracks the action.
    pub ticket: Option<String>,
    /// `low`, `medium`, `high` or `critical`.
    pub severity: Option<String>,
    pub retest: Option<ManualRetestArg>,
}

/// The run or the record that retested an action.
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualRetestArg {
    /// `run` (an automated run, by its experiment id) or `record` (a manual
    /// record, by its id).
    pub kind: String,
    pub id: String,
}

/// Who ran the test.
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualTesterArg {
    pub name: String,
    /// `internal` or `external`.
    pub kind: String,
}

/// The TLPT facts of a record of type `tlpt`: the attestation of the TLPT
/// authority and its date, and the test summary report.
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualTlptArg {
    pub attestation_ref: Option<String>,
    pub attested_at_ns: Option<i64>,
    pub summary_report_ref: Option<String>,
}

/// One link to what the exercise tested.
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualLinkArg {
    /// `function`, `entity`, `registry`, `run` or `gameday`.
    pub kind: String,
    pub linked_id: String,
    pub label: Option<String>,
}

/// Arguments for the `tumult_manual_create_draft` tool: the record body of
/// `POST /api/manual/experiments`.
#[macros::mcp_tool(
    name = "tumult_manual_create_draft",
    description = "Draft a record of the manual registry: the record body the daemon API takes (POST /api/manual/experiments), written to the lake as a draft. A field the body does not have is refused with its name, so a draft cannot carry a status or a reviewer. entered_by names the person the draft is entered for; that person may not sign it off. Structured content is {id, status}. Writes the lake, so the daemon must not hold it. Segregation of duties: an agent may draft, a person signs. No MCP tool submits, signs off, verifies or attaches evidence; a person submits the draft and other people sign it off in the daemon.",
    destructive_hint = false,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualCreateDraftTool {
    pub experiment_name: String,
    /// `gameday`, `tabletop`, `failover`, `pentest`, `tlpt`, `drill`,
    /// `restore`, `backup` or `other`.
    pub exercise_type: String,
    /// When the exercise ran, in unix nanoseconds.
    pub executed_at_ns: i64,
    pub hypothesis: String,
    pub method: String,
    /// `passed`, `failed`, `partial` or `inconclusive`.
    pub outcome_status: String,
    pub hypothesis_met: Option<bool>,
    pub findings: Option<String>,
    /// Actions as plain texts, as older clients send them; use `actions`.
    pub action_items: Option<Vec<String>>,
    pub target_system: Option<String>,
    pub target_environment: Option<String>,
    pub blast_radius: Option<String>,
    /// The actual RTO, in seconds.
    pub recovery_time_s: Option<f64>,
    pub duration_s: Option<f64>,
    /// The person the draft is entered for. They may not sign it off.
    pub entered_by: String,
    /// The statement the person who submits the record stands by.
    pub attestation: String,
    pub renewal_due_ns: Option<i64>,
    /// Requirement ids: a DORA requirement by its registry id, such as
    /// `dora.art24.6`, or a reference to another framework. A DORA id the
    /// registry does not know is refused with the nearest one.
    pub framework_refs: Option<Vec<String>>,
    /// The planned RTO, in seconds.
    pub rto_target_s: Option<f64>,
    /// The planned RPO: the seconds of committed writes from before the
    /// outage that may be lost.
    pub rpo_target_s: Option<f64>,
    /// The actual RPO: the seconds of committed writes that were lost.
    pub rpo_measured_s: Option<f64>,
    /// Whether the data was checked after the recovery: true complete,
    /// false loss or corruption found, absent not checked.
    pub data_verified: Option<bool>,
    pub participants: Option<Vec<ManualParticipantArg>>,
    /// The timeline, in the order the steps were taken.
    pub steps: Option<Vec<ManualStepArg>>,
    pub actions: Option<Vec<ManualActionArg>>,
    /// What the exercise tested.
    pub links: Option<Vec<ManualLinkArg>>,
    /// The plan the exercise fulfils once the record is verified.
    pub plan_id: Option<String>,
    /// How much the findings matter: `low`, `medium`, `high` or `critical`.
    pub findings_severity: Option<String>,
    /// Who ran the test, internal or external (DORA Art. 24(4)).
    pub tester: Option<ManualTesterArg>,
    /// The TLPT facts of a record of type `tlpt`.
    pub tlpt: Option<ManualTlptArg>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

/// Arguments for the `tumult_manual_plans_list` tool.
#[macros::mcp_tool(
    name = "tumult_manual_plans_list",
    description = "List the planned manual exercises (plans with an owner and a due date), the soonest due first. Filters: status (planned, fulfilled or cancelled) and target (the target system). Supports limit (default 100, max 1000) and offset; structured content is {items, total, offset, limit}. Reads the lake read-only, so the daemon must not hold it. Segregation of duties: an agent may draft, a person signs. No MCP tool submits, signs off, verifies or attaches evidence; a person does that in the daemon.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ManualPlansListTool {
    /// Only plans in this status: `planned`, `fulfilled` or `cancelled`.
    pub status: Option<String>,
    /// Only plans of this target system.
    pub target: Option<String>,
    /// Maximum number of plans returned (default 100, max 1000).
    #[serde(default = "default_list_limit")]
    pub limit: u64,
    /// Number of plans to skip before the returned page.
    #[serde(default)]
    pub offset: u64,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}
