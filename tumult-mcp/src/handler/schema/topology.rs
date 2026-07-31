//! Topology tool schemas: declared-topology import, the compliance map view,
//! the lineage matrix, and injection recommendations.

use rust_mcp_sdk::macros;

use super::default_store_path;

/// Arguments for the `tumult_topology_import` tool.
///
/// Exactly one of `toml_content`/`path` must be given; re-import replaces
/// the previously declared topology.
#[macros::mcp_tool(
    name = "tumult_topology_import",
    description = "Topology: import a declared service-topology TOML document (either inline toml_content or a file path — exactly one) into the persistent analytics store, replacing the previous declared topology. Opens the store read-write: the import persists svc: nodes and depends_on edges under a sentinel run id, so re-import is idempotent. The write is brief (one small transaction-sized delta) and the tool is Operator-gated. Structured content is {services, dependencies, service_ids}.",
    destructive_hint = false,
    read_only_hint = false,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct TopologyImportTool {
    /// Inline topology TOML (`[[service]]` blocks). Exactly one of
    /// `toml_content` and `path` must be given.
    pub toml_content: Option<String>,
    /// Path to a topology TOML file. Exactly one of `toml_content` and
    /// `path` must be given.
    pub path: Option<String>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

/// Arguments for the `tumult_topology_map` tool.
#[macros::mcp_tool(
    name = "tumult_topology_map",
    description = "Topology: render the compliance-aware service map — declared services with worst-of lineage state (OK / BROKEN / UNTESTED / UNKNOWN), depends_on edges, break causes, and ranked injection recommendations — as text (default), a Mermaid graph, or JSON. Reads the analytics store read-only. Structured content is {format, map} where map is the full view JSON.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct TopologyMapTool {
    /// Optional framework scope: one of `dora`, `nis2`, `pci-dss`,
    /// `iso-22301`, `iso-27001`, `soc2`, `basel-iii`.
    pub framework: Option<String>,
    /// Optional control scope (exact control id, e.g. `Art.25`).
    pub control: Option<String>,
    /// Output rendering: `text` (default), `mermaid`, or `json`.
    pub format: Option<String>,
    /// Include ranked injection recommendations (default true).
    pub recommend: Option<bool>,
    /// Maximum number of recommendations (default 3).
    pub limit: Option<u32>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

/// Arguments for the `tumult_compliance_lineage` tool.
#[macros::mcp_tool(
    name = "tumult_compliance_lineage",
    description = "Topology: the compliance lineage matrix — for each (regulatory article, service) pair the latest chaos evidence verdict (evidenced / broken / untested), with break attribution (deviation, fault, halting guard). Optionally scoped by framework, control, and service. Reads the analytics store read-only. Structured content is {cells, counts}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ComplianceLineageTool {
    /// Optional framework scope: one of `dora`, `nis2`, `pci-dss`,
    /// `iso-22301`, `iso-27001`, `soc2`, `basel-iii`.
    pub framework: Option<String>,
    /// Optional control scope (exact control id, e.g. `Art.25`).
    pub control: Option<String>,
    /// Optional service filter: a bare service name (`db`) or a full node
    /// id (`svc:db`).
    pub service: Option<String>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

/// Arguments for the `tumult_recommend_injection` tool.
#[macros::mcp_tool(
    name = "tumult_recommend_injection",
    description = "Topology: rank the next most valuable fault injections from the lineage matrix, declared depends_on topology, and plugin catalog. Deterministic and explained — every recommendation carries one human-readable reason per scoring factor. Reads the analytics store read-only. Structured content is {recommendations}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct RecommendInjectionTool {
    /// Optional framework scope: one of `dora`, `nis2`, `pci-dss`,
    /// `iso-22301`, `iso-27001`, `soc2`, `basel-iii`.
    pub framework: Option<String>,
    /// Maximum number of recommendations (default 3).
    pub limit: Option<u32>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}
