//! Agentic AI tool schemas.

use rust_mcp_sdk::macros;

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
    #[serde(default = "default_agentic_adapter")]
    pub adapter: String,
    #[serde(default = "default_agentic_scenario")]
    pub scenario: String,
    pub fault: Option<String>,
    pub contract: Option<String>,
}

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
    #[serde(default = "default_agentic_adapter")]
    pub adapter: String,
    #[serde(default = "default_agentic_scenario")]
    pub scenario: String,
    pub fault: Option<String>,
    pub contract: Option<String>,
}
fn default_agentic_adapter() -> String {
    "fake-http".into()
}
fn default_agentic_scenario() -> String {
    "malformed-json-recovery".into()
}
