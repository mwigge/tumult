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
pub(super) fn open_store(store_path: &str) -> Result<tumult_lake::AnalyticsStore, ToolError> {
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
    tumult_lake::AnalyticsStore::open(path).map_err(|e| ToolError::Store(e.to_string()))
}

/// Open the analytics store read-only, erroring cleanly if absent. Read-only
/// opens do not take the exclusive lock, so map/lineage/recommend queries
/// coexist with a running MCP server.
pub(super) fn open_store_ro(store_path: &str) -> Result<tumult_lake::AnalyticsStore, ToolError> {
    let path = Path::new(store_path);
    if !path.exists() {
        return Err(ToolError::NotFound(format!(
            "store not found: {store_path}"
        )));
    }
    tumult_lake::AnalyticsStore::open_read_only(path).map_err(|e| ToolError::Store(e.to_string()))
}

/// Everything the lineage/map/recommend tools read back from the store.
pub(crate) struct TopologyInputs {
    pub(crate) edges: Vec<EdgeRecord>,
    pub(crate) services: Vec<NodeSummary>,
    /// Services paired with their parsed attrs (`tier`/`owner`), for the map.
    pub(crate) services_with_attrs: Vec<(NodeSummary, serde_json::Value)>,
    pub(crate) articles: Vec<NodeSummary>,
    pub(crate) deviation_attrs: HashMap<String, serde_json::Value>,
    /// Declared `(src, dst)` pairs from `depends_on` edges.
    pub(crate) depends_on: Vec<(String, String)>,
}

impl TopologyInputs {
    /// The borrowed view `compute_lineage` consumes.
    pub(crate) fn lineage_input(&self) -> LineageInput<'_> {
        LineageInput {
            edges: &self.edges,
            services: &self.services,
            articles: &self.articles,
            deviation_attrs: &self.deviation_attrs,
        }
    }
}

/// Read every lineage/map input from the store in one pass.
pub(crate) fn gather_inputs(
    store: &tumult_lake::AnalyticsStore,
) -> Result<TopologyInputs, ToolError> {
    let edges = tumult_query::graph_edges_by_rels(store, LINEAGE_RELS)
        .map_err(|e| ToolError::Store(e.to_string()))?;
    let depends_on: Vec<(String, String)> = edges
        .iter()
        .filter(|e| e.rel == "depends_on")
        .map(|e| (e.src.clone(), e.dst.clone()))
        .collect();

    let services_with_attrs: Vec<(NodeSummary, serde_json::Value)> =
        tumult_query::graph_nodes_with_attrs(store, "service")
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

    let articles: Vec<NodeSummary> =
        tumult_query::graph_nodes_with_attrs(store, "compliance_article")
            .map_err(|e| ToolError::Store(e.to_string()))?
            .into_iter()
            .map(|n| NodeSummary {
                id: n.id,
                kind: "compliance_article".into(),
                label: n.label,
            })
            .collect();

    let deviation_attrs: HashMap<String, serde_json::Value> =
        tumult_query::graph_nodes_with_attrs(store, "deviation")
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
pub(crate) fn recommendations_for(
    store: &tumult_lake::AnalyticsStore,
    inputs: &TopologyInputs,
    lineage: &[LineageCell],
    limit: usize,
) -> Result<Vec<Recommendation>, ToolError> {
    let available = available_actions();
    let tested: HashSet<String> =
        tumult_query::tested_action_names(store).map_err(|e| ToolError::Store(e.to_string()))?;
    let strength = article_strength();
    Ok(recommend(
        &RecommendationInput {
            criticality: &criticality_from_env(),
            lineage,
            depends_on: &inputs.depends_on,
            available_actions: &available,
            tested_action_names: &tested,
            article_strength: &strength,
        },
        limit,
    ))
}

/// Observed-traffic criticality per service, from `TUMULT_CRITICALITY_FILE`
/// (a JSON object: bare service name or `svc:` id → relative rate, e.g.
/// spans/min extracted from an `OTel` backend). Absent/invalid = empty map =
/// neutral factor. File-based on purpose: the derivation from `OTel` happens
/// in whatever pipeline owns your telemetry (see the autopilot guide for
/// the SigNoz/ClickHouse one-liner); tumult consumes a reviewable artifact.
pub(crate) fn criticality_from_env() -> std::collections::HashMap<String, f64> {
    let Ok(path) = std::env::var("TUMULT_CRITICALITY_FILE") else {
        return std::collections::HashMap::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return std::collections::HashMap::new();
    };
    let Ok(map) = serde_json::from_str::<std::collections::HashMap<String, f64>>(&text) else {
        return std::collections::HashMap::new();
    };
    map.into_iter()
        .map(|(k, v)| {
            let id = if k.starts_with("svc:") {
                k
            } else {
                format!("svc:{k}")
            };
            (id, v)
        })
        .collect()
}
