//! Core tool schemas: experiment execution, validation, analysis, journals,
//! discovery, reporting, compliance, trends, and access.

use rust_mcp_sdk::macros;

use super::{default_list_limit, default_store_path};

/// Arguments for the `tumult_run_experiment` tool.
///
/// The journal is persisted and auto-ingested into the analytics store
/// unless `no_ingest` is set.
#[macros::mcp_tool(
    name = "tumult_run_experiment",
    description = "Execute a Tumult chaos experiment. Persists the journal (journal_path, default journal.toon in the workspace root), auto-ingests it into the analytics store unless no_ingest is set, and returns the run as JSON (format=json, default) or the journal as TOON (format=toon).",
    destructive_hint = true,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = true
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct RunExperimentTool {
    /// Path to the experiment `.toon` file.
    pub experiment_path: String,
    /// One of `on-deviation` (default), `always`, `never`.
    #[serde(default = "default_strategy")]
    pub rollback_strategy: String,
    /// Where to write the journal, relative to the workspace root.
    /// Defaults to `journal.toon` (CLI parity).
    pub journal_path: Option<String>,
    /// Skip analytics-store ingestion (parity with the CLI `--no-ingest`).
    #[serde(default)]
    pub no_ingest: bool,
    /// Analytics store the journal is ingested into.
    #[serde(default = "default_store_path")]
    pub store_path: String,
    /// Text content format: `json` (default) or `toon`.
    #[serde(default = "default_journal_format")]
    pub format: String,
}
fn default_strategy() -> String {
    "on-deviation".into()
}
fn default_journal_format() -> String {
    "json".into()
}

/// Arguments for the `tumult_validate` tool.
#[macros::mcp_tool(
    name = "tumult_validate",
    description = "Validate an experiment file for syntax and provider support.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ValidateTool {
    /// Path to the experiment `.toon` file.
    pub experiment_path: String,
}

/// Arguments for the `tumult_analyze` tool.
#[macros::mcp_tool(
    name = "tumult_analyze",
    description = "SQL query over experiment journals via embedded DuckDB.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AnalyzeTool {
    /// Journal `.toon` file or directory of journals to query.
    pub journals_path: String,
    /// SQL query executed over the journals via DuckDB.
    pub query: String,
}

/// Arguments for the `tumult_read_journal` tool.
///
/// Returned text is capped at 512 KiB.
#[macros::mcp_tool(
    name = "tumult_read_journal",
    description = "Read a journal file and return it as JSON (format=json, default) or raw TOON (format=toon). Set summary=true for a compact summary instead of the full journal. Text content is capped at 512 KiB.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ReadJournalTool {
    /// Path to the journal `.toon` file.
    pub journal_path: String,
    /// Text content format: `json` (default) or `toon`.
    #[serde(default = "default_journal_format")]
    pub format: String,
    /// Return only a compact summary (title, status, timing, counts).
    #[serde(default)]
    pub summary: bool,
}

/// Arguments for the `tumult_list_journals` tool.
#[macros::mcp_tool(
    name = "tumult_list_journals",
    description = "List .toon journal files in a directory (sorted). Supports limit (default 100, max 1000) and offset; structured content is {items, total, offset, limit}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ListJournalsTool {
    /// Directory to list. `directory` is accepted as a legacy alias.
    #[serde(alias = "directory")]
    pub path: String,
    /// Maximum number of entries returned (default 100, max 1000).
    #[serde(default = "default_list_limit")]
    pub limit: u64,
    /// Number of entries to skip before the returned page.
    #[serde(default)]
    pub offset: u64,
}

/// Arguments for the `tumult_discover` tool (takes none).
#[macros::mcp_tool(
    name = "tumult_discover",
    description = "List all Tumult plugins, actions, and probes.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct DiscoverTool {}

/// Arguments for the `tumult_create_experiment` tool.
#[macros::mcp_tool(
    name = "tumult_create_experiment",
    description = "Create a new experiment from a template.",
    destructive_hint = false,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct CreateExperimentTool {
    /// Where to write the new experiment file.
    pub output_path: String,
    /// Optional plugin to template the experiment from.
    pub plugin: Option<String>,
}

/// Arguments for the `tumult_query_traces` tool.
#[macros::mcp_tool(
    name = "tumult_query_traces",
    description = "Query trace data from a journal — returns activity spans with trace/span IDs for observability correlation.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct QueryTracesTool {
    /// Path to the journal `.toon` file.
    pub journal_path: String,
}

/// Arguments for the `tumult_store_stats` tool.
#[macros::mcp_tool(
    name = "tumult_store_stats",
    description = "Get persistent analytics store statistics — experiment count, activity count, schema version, file size.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct StoreStatsTool {
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

/// Arguments for the `tumult_analyze_store` tool.
#[macros::mcp_tool(
    name = "tumult_analyze_store",
    description = "SQL query over the persistent analytics store (accumulated history from all runs).",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AnalyzeStoreTool {
    /// SQL query executed over the analytics store.
    pub query: String,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

/// Arguments for the `tumult_list_experiments` tool.
#[macros::mcp_tool(
    name = "tumult_list_experiments",
    description = "List all .toon experiment files recursively from the workspace or a given path (sorted by relative path). Supports limit (default 100, max 1000) and offset; structured content is {items, total, offset, limit}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ListExperimentsTool {
    /// Optional subdirectory to search within (relative to workspace root).
    pub path: Option<String>,
    /// Maximum number of entries returned (default 100, max 1000).
    #[serde(default = "default_list_limit")]
    pub limit: u64,
    /// Number of entries to skip before the returned page.
    #[serde(default)]
    pub offset: u64,
}

/// Arguments for the `tumult_report` tool.
///
/// Inline output is capped at 512 KiB; HTML/PDF reports are CLI-only.
#[macros::mcp_tool(
    name = "tumult_report",
    description = "Render an experiment journal as a report: format=json (raw journal JSON, default) or format=junit (JUnit XML, one testcase per activity). With output_path the report is written inside the workspace and the path is returned; otherwise the content is returned inline (capped at 512 KiB). HTML/PDF reports are CLI-only.",
    destructive_hint = false,
    read_only_hint = false,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ReportTool {
    /// Path to the journal `.toon` file.
    pub journal_path: String,
    /// Report format: `json` (default) or `junit`.
    #[serde(default = "default_journal_format")]
    pub format: String,
    /// Where to write the report, relative to the workspace root. When
    /// omitted the report content is returned inline.
    pub output_path: Option<String>,
}

/// Arguments for the `tumult_compliance` tool.
#[macros::mcp_tool(
    name = "tumult_compliance",
    description = "Regulatory compliance summary over journals (a .toon journal file or a directory of them): pass rate, recovery compliance, and verdict for a target framework. Valid frameworks: dora, nis2, pci-dss, iso-22301, iso-27001, soc2, basel-iii.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ComplianceTool {
    /// Journal `.toon` file or directory containing journal files.
    pub journals_path: String,
    /// Target framework: one of `dora`, `nis2`, `pci-dss`, `iso-22301`,
    /// `iso-27001`, `soc2`, `basel-iii`.
    pub framework: String,
}

/// Arguments for the `tumult_trend` tool.
#[macros::mcp_tool(
    name = "tumult_trend",
    description = "Cross-run metric trend over journals (a .toon journal file or a directory of them): metric one of resilience_score (default), duration_ms, estimate_accuracy, method_step_count; optional last window (e.g. 30d) and target title filter. Returns time-ordered points and a direction verdict.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct TrendTool {
    /// Journal `.toon` file or directory containing journal files.
    pub journals_path: String,
    /// Metric to track: `resilience_score` (default), `duration_ms`,
    /// `estimate_accuracy`, or `method_step_count`.
    #[serde(default = "default_trend_metric")]
    pub metric: String,
    /// Time window in days (e.g. `30d`).
    pub last: Option<String>,
    /// Case-insensitive experiment-title substring filter.
    pub target: Option<String>,
}
fn default_trend_metric() -> String {
    "resilience_score".into()
}

/// Arguments for the `tumult_agents` tool (takes none).
#[macros::mcp_tool(
    name = "tumult_agents",
    description = "List agent CLI adapters (claude-code, codex) with install, version, and auth state. Probes local binaries by spawning short version checks (no network access, no prompts).",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AgentsTool {}

/// Arguments for the `tumult_whoami` tool (takes none).
#[macros::mcp_tool(
    name = "tumult_whoami",
    description = "Return the caller's resolved access role. Structured content is {role: 'viewer'|'operator'|'approver'|'admin', authenticated: bool}: `role` is the role this request's bearer token maps to (viewer = read-only tools only, operator-or-above = every tool including fault injection/execution), and `authenticated` is true when a configured token validated the request (false in loopback open mode, where every caller has full access without a token). Read-only and viewer-callable — a client uses it to discover its own permissions and adapt its UI to them.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct WhoamiTool {}
