//! `GameDay` tool schemas.

use rust_mcp_sdk::macros;

use super::default_list_limit;

#[macros::mcp_tool(
    name = "tumult_gameday_run",
    description = "Run a GameDay — execute all experiments in a .gameday.toon file under shared load. Returns resilience score and compliance status.",
    destructive_hint = true,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = true
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayRunTool {
    /// Path to the `.gameday.toon` file.
    pub gameday_path: String,
}

#[macros::mcp_tool(
    name = "tumult_gameday_analyze",
    description = "Analyze a completed GameDay journal — returns resilience score, per-experiment results, and compliance article mapping.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayAnalyzeTool {
    /// Path to the `.gameday.toon` file (reads the .journal.toon alongside it).
    pub gameday_path: String,
}

#[macros::mcp_tool(
    name = "tumult_gameday_create",
    description = "Create a .gameday.toon campaign file (<name>.gameday.toon in the workspace root) from experiment paths, with optional shared load config (load_tool k6 or jmeter, load_script, load_vus) and compliance framework mapping. Fails if the file already exists.",
    destructive_hint = false,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayCreateTool {
    /// `GameDay` name; the file is written as `<name>.gameday.toon`.
    pub name: String,
    /// Experiment `.toon` paths referenced by the campaign (resolved
    /// relative to the gameday file when it is run).
    pub experiments: Vec<String>,
    /// Load tool to run during the campaign: `k6`, `jmeter`, or `none`
    /// (default: no load).
    pub load_tool: Option<String>,
    /// Load script path recorded in the load config.
    pub load_script: Option<String>,
    /// Virtual users for the load test.
    pub load_vus: Option<u32>,
    /// Compliance framework to map: one of `dora`, `nis2`, `pci-dss`,
    /// `iso-22301`, `iso-27001`, `soc2`, `basel-iii`.
    pub framework: Option<String>,
}

#[macros::mcp_tool(
    name = "tumult_gameday_list",
    description = "List available GameDay files (.gameday.toon) in the workspace (sorted by path). Supports limit (default 100, max 1000) and offset; structured content is {items, total, offset, limit}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayListTool {
    /// Optional subdirectory to search within.
    pub path: Option<String>,
    /// Maximum number of entries returned (default 100, max 1000).
    #[serde(default = "default_list_limit")]
    pub limit: u64,
    /// Number of entries to skip before the returned page.
    #[serde(default)]
    pub offset: u64,
}
