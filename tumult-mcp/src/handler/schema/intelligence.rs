//! Intelligence tool schemas (agent reasoning): recommend and coverage.

use rust_mcp_sdk::macros;

use super::default_store_path;

/// Arguments for the `tumult_recommend` tool.
///
/// Setting `agent` enables agent-CLI enhancement, which may call the
/// agent's model API over the network.
#[macros::mcp_tool(
    name = "tumult_recommend",
    description = "Recommend what to test next — deterministic heuristics over coverage gaps, failure patterns, and stale experiments (shared with `tumult recommend`). Optionally enhance with a local agent CLI adapter (agent=claude-code|codex, plus agent_model, agent_timeout_secs, generate_experiments_dir): this spawns the local agent binary, which may call its model API over the network. Generated experiments pass a parse+validate gate before being written into generate_experiments_dir.",
    destructive_hint = false,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = true
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct RecommendTool {
    #[serde(default = "default_store_path")]
    pub store_path: String,
    /// Optional operator goal woven into the recommendations.
    pub goal: Option<String>,
    /// Model label recorded in the deterministic metadata.
    pub model: Option<String>,
    /// Include a draft TOON experiment when one is proposed (default true).
    #[serde(default = "default_include_draft")]
    pub include_draft: bool,
    /// Output rendering: `text` (default) or `json`.
    #[serde(default = "default_recommend_format")]
    pub format: String,
    /// Agent CLI adapter name (e.g. `claude-code`, `codex`); enables
    /// agent-enhanced recommendations.
    pub agent: Option<String>,
    /// Model override passed to the agent CLI (requires `agent`).
    pub agent_model: Option<String>,
    /// Agent CLI timeout in seconds (default 120; only used with `agent`).
    #[serde(default = "default_agent_timeout_secs")]
    pub agent_timeout_secs: u64,
    /// Directory (relative to the workspace root) to write validated
    /// agent-proposed experiments into (requires `agent`).
    pub generate_experiments_dir: Option<String>,
}
fn default_include_draft() -> bool {
    true
}
fn default_recommend_format() -> String {
    "text".into()
}
fn default_agent_timeout_secs() -> u64 {
    tumult_agent_cli::adapter::DEFAULT_TIMEOUT.as_secs()
}

/// Arguments for the `tumult_coverage` tool.
#[macros::mcp_tool(
    name = "tumult_coverage",
    description = "Coverage report — which plugins, actions, and targets have been tested vs available. Shows per-plugin test status (FULL/PARTIAL/NONE) and store statistics.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct CoverageTool {
    #[serde(default = "default_store_path")]
    pub store_path: String,
}
