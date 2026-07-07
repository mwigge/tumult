//! Store access and input gathering shared by the topology tools: read-only
//! vs read-write opens, the lineage input bundle, and the catalog/citation
//! lookups that feed recommendation scoring.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use tumult_graph::lineage::{LineageCell, LineageInput};
use tumult_graph::recommend::{recommend, Recommendation, RecommendationInput};
use tumult_graph::{AvailableAction, EdgeRecord, NodeSummary};

use crate::error::ToolError;

/// Edge relations lineage needs; extras are ignored by `compute_lineage`.
const LINEAGE_RELS: &[&str] = &[
    "targets",
    "yielded",
    "exhibited",
    "evidences",
    "maps_to_compliance",
    "caused_by",
    "depends_on",
];

/// Open the analytics store read-write at `store_path`, erroring cleanly if
/// absent. Used only by `topology_import`, which persists `depends_on` edges;
/// it takes the exclusive lock and so contends with a running server.
pub(super) fn open_store(store_path: &str) -> Result<tumult_analytics::AnalyticsStore, ToolError> {
    // Unlike the run-data tools, topology import is legitimately the FIRST
    // write a fresh deployment performs — create the store when absent
    // (parent directory must exist; that typo-guard stays).
    let path = Path::new(store_path);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            return Err(ToolError::NotFound(format!(
                "store directory not found: {}",
                parent.display()
            )));
        }
    }
    tumult_analytics::AnalyticsStore::open(path).map_err(|e| ToolError::Store(e.to_string()))
}

/// Open the analytics store read-only, erroring cleanly if absent. Read-only
/// opens do not take the exclusive lock, so map/lineage/recommend queries
/// coexist with a running MCP server.
pub(super) fn open_store_ro(
    store_path: &str,
) -> Result<tumult_analytics::AnalyticsStore, ToolError> {
    let path = Path::new(store_path);
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "store not found: {store_path}"
        )));
    }
    tumult_analytics::AnalyticsStore::open_read_only(path)
        .map_err(|e| ToolError::Store(e.to_string()))
}

/// Everything the lineage/map/recommend tools read back from the store.
pub(super) struct TopologyInputs {
    pub(super) edges: Vec<EdgeRecord>,
    pub(super) services: Vec<NodeSummary>,
    /// Services paired with their parsed attrs (`tier`/`owner`), for the map.
    pub(super) services_with_attrs: Vec<(NodeSummary, serde_json::Value)>,
    pub(super) articles: Vec<NodeSummary>,
    pub(super) deviation_attrs: HashMap<String, serde_json::Value>,
    /// Declared `(src, dst)` pairs from `depends_on` edges.
    pub(super) depends_on: Vec<(String, String)>,
}

impl TopologyInputs {
    /// The borrowed view `compute_lineage` consumes.
    pub(super) fn lineage_input(&self) -> LineageInput<'_> {
        LineageInput {
            edges: &self.edges,
            services: &self.services,
            articles: &self.articles,
            deviation_attrs: &self.deviation_attrs,
        }
    }
}

/// Read every lineage/map input from the store in one pass.
pub(super) fn gather_inputs(
    store: &tumult_analytics::AnalyticsStore,
) -> Result<TopologyInputs, ToolError> {
    let edges = store
        .graph_edges_by_rels(LINEAGE_RELS)
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let depends_on: Vec<(String, String)> = edges
        .iter()
        .filter(|e| e.rel == "depends_on")
        .map(|e| (e.src.clone(), e.dst.clone()))
        .collect();

    let services_with_attrs: Vec<(NodeSummary, serde_json::Value)> = store
        .graph_nodes_with_attrs("service")
        .map_err(|e| ToolError::Store(e.to_string()))?
        .into_iter()
        .map(|n| {
            let attrs = serde_json::from_str(&n.attrs).unwrap_or(serde_json::json!({}));
            (
                NodeSummary {
                    id: n.id,
                    kind: "service".into(),
                    label: n.label,
                },
                attrs,
            )
        })
        .collect();
    let services: Vec<NodeSummary> = services_with_attrs
        .iter()
        .map(|(node, _)| node.clone())
        .collect();

    let articles: Vec<NodeSummary> = store
        .graph_nodes_with_attrs("compliance_article")
        .map_err(|e| ToolError::Store(e.to_string()))?
        .into_iter()
        .map(|n| NodeSummary {
            id: n.id,
            kind: "compliance_article".into(),
            label: n.label,
        })
        .collect();

    let deviation_attrs: HashMap<String, serde_json::Value> = store
        .graph_nodes_with_attrs("deviation")
        .map_err(|e| ToolError::Store(e.to_string()))?
        .into_iter()
        .map(|n| {
            let attrs = serde_json::from_str(&n.attrs).unwrap_or(serde_json::json!({}));
            (n.id, attrs)
        })
        .collect();

    Ok(TopologyInputs {
        edges,
        services,
        services_with_attrs,
        articles,
        deviation_attrs,
        depends_on,
    })
}

/// Canonicalize an optional framework filter (`dora` → `DORA`), rejecting
/// unknown values with the registry's error message.
pub(super) fn canonical_framework(
    framework: Option<&str>,
) -> Result<Option<&'static str>, ToolError> {
    framework
        .map(|fw| {
            tumult_core::compliance::ComplianceFramework::parse(fw)
                .map(tumult_core::compliance::ComplianceFramework::as_report_str)
                .map_err(ToolError::InvalidInput)
        })
        .transpose()
}

/// Article id → citation strength, from the shared citation registry.
fn article_strength() -> HashMap<String, String> {
    tumult_core::compliance::CITATIONS
        .iter()
        .map(|c| {
            (
                tumult_graph::compliance_article_id(c.framework, c.control_id),
                c.strength.as_str().to_string(),
            )
        })
        .collect()
}

/// Available actions from the plugin catalog (same mapping `coverage_gaps`
/// uses).
fn available_actions() -> Vec<AvailableAction> {
    let plugins = tumult_plugin::discovery::discover_all_plugins().unwrap_or_default();
    plugins
        .iter()
        .flat_map(|p| {
            p.actions
                .iter()
                .map(move |a| AvailableAction::new(&p.name, &a.name))
        })
        .collect()
}

/// Compute ranked recommendations for a lineage matrix.
pub(super) fn recommendations_for(
    store: &tumult_analytics::AnalyticsStore,
    inputs: &TopologyInputs,
    lineage: &[LineageCell],
    limit: usize,
) -> Result<Vec<Recommendation>, ToolError> {
    let available = available_actions();
    let tested: HashSet<String> = store
        .tested_action_names()
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let strength = article_strength();
    Ok(recommend(
        &RecommendationInput {
            lineage,
            depends_on: &inputs.depends_on,
            available_actions: &available,
            tested_action_names: &tested,
            article_strength: &strength,
        },
        limit,
    ))
}
