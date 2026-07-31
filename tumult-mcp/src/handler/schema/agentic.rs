//! Agentic AI tool schemas.

use rust_mcp_sdk::macros;

/// Arguments for the `tumult_agentic_list_scenarios` tool (takes none).
#[macros::mcp_tool(
    name = "tumult_agentic_list_scenarios",
    description = "List deterministic agentic AI fault-injection scenario packs. Returns metadata only; no prompts or raw payloads.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgenticListScenariosTool {}

/// Arguments for the `tumult_agentic_smoke` tool.
///
/// Metadata only — raw payloads are neither accepted nor returned.
#[macros::mcp_tool(
    name = "tumult_agentic_smoke",
    description = "Run a deterministic local agentic AI smoke check with clear fault, contract, expected/actual, and diagnostic feedback. Metadata only; raw payloads are not accepted or returned.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgenticSmokeTool {
    /// Agent adapter to run against (default `fake-http`).
    #[serde(default = "default_agentic_adapter")]
    pub adapter: String,
    /// Scenario pack to exercise (default `malformed-json-recovery`).
    #[serde(default = "default_agentic_scenario")]
    pub scenario: String,
    /// Optional fault to inject.
    pub fault: Option<String>,
    /// Optional contract to check against.
    pub contract: Option<String>,
}

/// Arguments for the `tumult_agentic_run_experiment` tool.
///
/// Metadata only — raw payloads are neither accepted nor returned.
#[macros::mcp_tool(
    name = "tumult_agentic_run_experiment",
    description = "Run a deterministic bundled agentic AI experiment with input schema validation. Metadata only; raw payloads are not accepted or returned.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgenticRunExperimentTool {
    /// Agent adapter to run against (default `fake-http`).
    #[serde(default = "default_agentic_adapter")]
    pub adapter: String,
    /// Scenario pack to exercise (default `malformed-json-recovery`).
    #[serde(default = "default_agentic_scenario")]
    pub scenario: String,
    /// Optional fault to inject.
    pub fault: Option<String>,
    /// Optional contract to check against.
    pub contract: Option<String>,
}
fn default_agentic_adapter() -> String {
    "fake-http".into()
}
fn default_agentic_scenario() -> String {
    "malformed-json-recovery".into()
}
