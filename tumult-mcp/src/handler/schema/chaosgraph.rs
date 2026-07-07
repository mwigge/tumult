//! ChaosGraph tool schemas.

use rust_mcp_sdk::macros;

use super::default_store_path;

#[macros::mcp_tool(
    name = "tumult_chaosgraph_query",
    description = "ChaosGraph: list graph node ids + one-line summaries for a kind (experiment, fault, service, journal, deviation, compliance_article, coverage_gap, fault_domain) from the persistent analytics store, optionally filtered by a case-insensitive label substring. Small, token-efficient output. Structured content is {kind, count, nodes:[{id,kind,label}]}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ChaosGraphQueryTool {
    /// Node kind: `experiment`, `fault`, `service`, `journal`, `deviation`,
    /// `compliance_article`, `coverage_gap`, or `fault_domain`.
    pub kind: String,
    /// Optional case-insensitive label substring filter.
    pub filter: Option<String>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

#[macros::mcp_tool(
    name = "tumult_chaosgraph_neighbors",
    description = "ChaosGraph: return the ego sub-graph of a node (its neighbourhood within `depth`, default 1) as compact (src)-[rel]->(dst) tuples plus node labels. Optionally filter to a single relation (targets, injects, yielded, observed_on, exhibited, evidences, maps_to_compliance, gap_in, depends_on, caused_by). Structured content is {node_id, depth, nodes:[{id,kind,label}], edges:[{src,rel,dst}]}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ChaosGraphNeighborsTool {
    /// The node id to centre on (e.g. `exp:<title>`, `fault:<plugin>::<fn>`).
    pub node_id: String,
    /// Optional relation filter: `targets`, `injects`, `yielded`,
    /// `observed_on`, `exhibited`, `evidences`, `maps_to_compliance`,
    /// `gap_in`, `depends_on`, or `caused_by`.
    pub rel: Option<String>,
    /// Neighbourhood radius (default 1).
    #[serde(default = "default_graph_depth")]
    pub depth: u32,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}
fn default_graph_depth() -> u32 {
    1
}

#[macros::mcp_tool(
    name = "tumult_chaosgraph_coverage_gaps",
    description = "ChaosGraph: list plugin-catalog actions that have never appeared in a tested run (coverage gaps), optionally filtered by fault domain (plugin substring). When a framework is given (dora, nis2, pci-dss, iso-22301, iso-27001, soc2, basel-iii), also lists that framework's articles still lacking any evidence edge. Refreshes the CoverageGap/FaultDomain nodes + gap_in edges in the store's graph. Structured content is {count, gaps:[{id,plugin,action,domain}], framework?, unevidenced_articles?}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ChaosGraphCoverageGapsTool {
    /// Optional framework filter: one of `dora`, `nis2`, `pci-dss`,
    /// `iso-22301`, `iso-27001`, `soc2`, `basel-iii`. When set, the response
    /// also lists that framework's still-unevidenced articles.
    pub framework: Option<String>,
    /// Optional fault-domain (plugin) filter — case-insensitive substring of
    /// the plugin name (e.g. `tumult-net`).
    pub domain: Option<String>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

#[macros::mcp_tool(
    name = "tumult_chaosgraph_cypher",
    description = "ChaosGraph: run an arbitrary READ-ONLY openCypher query over the whole graph (node labels = kinds: experiment, fault, service, journal, deviation, compliance_article, coverage_gap, fault_domain; relationship types: targets, injects, yielded, observed_on, exhibited, evidences, maps_to_compliance, gap_in, depends_on, caused_by; node props: id, label + attrs; edge props: run_id, ts + attrs). The graph is snapshotted from the analytics store into an in-memory engine per call — DuckDB stays the source of truth. Mutating clauses are rejected; rows are capped (default 500). Structured content is {columns, rows, truncated, graph}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ChaosGraphCypherTool {
    /// The openCypher query (MATCH/RETURN/WHERE/ORDER BY/LIMIT; no mutations).
    pub query: String,
    /// Maximum rows returned (default 500).
    pub row_cap: Option<u32>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}
