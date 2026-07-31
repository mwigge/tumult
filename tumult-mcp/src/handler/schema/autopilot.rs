//! Autopilot tool schemas: one pass of the decision loop, the status/log
//! readback, the human approve/deny response, and the Parquet export.

use rust_mcp_sdk::macros;

use super::default_store_path;

/// Arguments for the `tumult_autopilot_run` tool.
///
/// Every decision is persisted before any action runs; with `execute`
/// unset or false the pass only decides and records — nothing is injected.
#[macros::mcp_tool(
    name = "tumult_autopilot_run",
    description = "Autopilot: run ONE pass of the decision loop over the given policy TOML — assemble injection candidates from the compliance lineage, gate each against the policy, and persist every decision (enact / downgrade / propose / veto). AUDIT-BEFORE-ACT contract: each decision record is written to the analytics store BEFORE any action runs, so a crash mid-loop leaves the truthful partial record. By default (execute=false) the pass only decides and records — NOTHING is injected. Setting execute=true ACTUALLY INJECTS FAULTS: each enact verdict runs its policy-bound playbook experiment against the real target. Even with execute=false, guard probes run once against the target during pre-flight; no faults are injected. Every pass creates new decision records, so repeated calls are not idempotent. Structured content is {decisions, enacted, policy_hash, executed}.",
    destructive_hint = true,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = true
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AutopilotRunTool {
    /// Path to the autopilot policy TOML (`[autopilot]` table). The policy
    /// is the operator's contract with the gate; its sha256 is persisted
    /// with every decision.
    pub policy_path: String,
    /// When true, enact verdicts run their playbook experiments — real
    /// fault injection. Default false: decide and record only.
    pub execute: Option<bool>,
    /// Maximum candidates gated in this pass (default 3, clamped 1-10).
    pub limit: Option<u32>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

/// Arguments for the `tumult_autopilot_status` tool.
#[macros::mcp_tool(
    name = "tumult_autopilot_status",
    description = "Autopilot: list recorded decisions with their latest lifecycle event (run_started / run_completed / run_failed / human_approved / human_denied), newest first, optionally filtered by verdict. Reads the analytics store read-only. Structured content is {decisions, count}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AutopilotStatusTool {
    /// Optional verdict filter: one of `enact`, `downgrade`, `propose`,
    /// `veto`.
    pub verdict: Option<String>,
    /// Maximum number of decisions returned (default 20).
    pub limit: Option<u32>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

/// Arguments for the `tumult_autopilot_respond` tool.
///
/// Approving re-evaluates the full gate against current state before the
/// playbook experiment runs; a decision takes exactly one response.
#[macros::mcp_tool(
    name = "tumult_autopilot_respond",
    description = "Autopilot: record the human response to a proposed/downgraded decision. approve=true runs the decision's playbook experiment (real fault injection, journaled and ingested like any run) — but only after the FULL gate is re-evaluated against CURRENT state: supply the same policy TOML via policy_path, and if the policy hash changed or any gate rule now vetoes or downgrades (enrollment, concurrency, cooldown, guard telemetry pre-flight, …), execution is refused and the refusal is recorded in the audit trail. No stale approval ever executes. approve=false records the veto feedback the autonomy ladder consumes — denials keep a fault class from graduating to unattended enact. Either response is appended as an event BEFORE any experiment runs, and a decision takes exactly one response. Structured content is {decision_id, action}.",
    destructive_hint = true,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = true
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AutopilotRespondTool {
    /// The decision id to respond to (from `tumult_autopilot_status`).
    pub decision_id: String,
    /// true = approve (runs the playbook experiment), false = deny
    /// (records the veto feedback).
    pub approve: bool,
    /// Optional human reason, persisted with the response event.
    pub reason: Option<String>,
    /// Path to the autopilot policy TOML the decision was gated with.
    /// Required when approve=true: the gate is re-evaluated against current
    /// state before the playbook runs, and the policy hash must match the
    /// hash recorded with the decision.
    pub policy_path: Option<String>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

/// Arguments for the `tumult_autopilot_export` tool.
#[macros::mcp_tool(
    name = "tumult_autopilot_export",
    description = "Autopilot: export the decision and event tables as a Parquet archive — writes autopilot_decisions.parquet and autopilot_events.parquet into the given directory (overwriting previous exports there). Structured content is {dir}.",
    destructive_hint = false,
    read_only_hint = false,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AutopilotExportTool {
    /// Output directory for the Parquet files (created if absent).
    pub dir: String,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

/// Arguments for the `tumult_autopilot_notify` tool.
///
/// Insert-only — recording the change event runs nothing itself.
#[macros::mcp_tool(
    name = "tumult_autopilot_notify",
    description = "Autopilot: record an external change event (deploy, config change) against a service. The next autopilot pass treats the service's evidence as invalidated and proposes revalidation via its playbook — change-triggered evidence invalidation, not just time-triggered. Insert-only; nothing runs from this call.",
    destructive_hint = false,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AutopilotNotifyTool {
    /// Service name (bare or `svc:` id) whose evidence the change invalidates.
    pub service: String,
    /// What reported the change (e.g. `deploy-webhook`, `config-watcher`).
    pub source: String,
    /// Optional human-readable detail about what changed.
    pub detail: Option<String>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}
