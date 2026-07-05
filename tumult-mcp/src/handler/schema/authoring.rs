//! Authoring tool schemas (fault catalog + scaffolding).

use rust_mcp_sdk::macros;

#[macros::mcp_tool(
    name = "tumult_fault_catalog",
    description = "Return the live fault catalog derived from the installed plugins: fault domains (Network, Database, Resource, State, Process, Container, Time, …), each with their actions/probes and documented arguments. Structured content is {action_count, domains:[{domain, label, actions:[{plugin, name, description, kind, args:[{name, required, description}]}]}]}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct FaultCatalogTool {}

#[macros::mcp_tool(
    name = "tumult_scaffold_experiment",
    description = "Scaffold a validated experiment from a chosen fault action. Give `plugin`+`action` (or a fully-qualified `action` as plugin::action), an `args` object, a `target`, an optional steady-state probe (probe_command or probe_url, plus probe_expect), and an optional title. Read-only w.r.t. the store — pure generation. Structured content is {action, toon, valid, validation_error?}: the generated TOON and whether it passes `tumult validate`.",
    destructive_hint = false,
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ScaffoldExperimentTool {
    /// Owning plugin, e.g. `tumult-network`. Optional when `action` is
    /// fully-qualified as `plugin::action`.
    pub plugin: Option<String>,
    /// Action name (e.g. `add-latency`) or `plugin::action`.
    pub action: String,
    /// Argument values as a JSON object (name → value).
    #[serde(default)]
    pub args: serde_json::Map<String, serde_json::Value>,
    /// Logical target of the fault (host / container / service).
    pub target: String,
    /// Shell command for the steady-state probe (health check).
    pub probe_command: Option<String>,
    /// HTTP URL for the steady-state probe (checked with `curl`).
    pub probe_url: Option<String>,
    /// Regex the probe output/response must match.
    pub probe_expect: Option<String>,
    /// Experiment title (defaults to `<action> — <target>`).
    pub title: Option<String>,
}
