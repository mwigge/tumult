//! Coverage-gap derivation: catalog actions never exercised by a tested run.
//!
//! An [`AvailableAction`] is a fault primitive advertised by a plugin. A gap is
//! an available action no stored run injected. This module owns only the
//! *pure* derivation — the caller supplies the plugin catalog (from
//! `tumult-plugin` discovery) and the [`TestedFaults`] (from the analytics
//! store), keeping `tumult-graph` free of both a database handle and a plugin
//! dependency.
//!
//! An action `plugin::action` is tested when a run ingested with its
//! definition injected the fault `plugin::action`: its method holds an action
//! that calls that function of that plugin. A step's name, a process
//! activity, a rollback, and a run ingested without its definition test no
//! catalog action.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::map::plugin_fault_id;
use crate::model::{Edge, EdgeRel, GraphDelta, Node, NodeKind};

/// A fault primitive advertised by a plugin's manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableAction {
    /// The owning plugin / fault domain (e.g. `tumult-net`).
    pub plugin: String,
    /// The action / fault name (e.g. `inject_latency`).
    pub action: String,
}

impl AvailableAction {
    /// Construct an available action.
    pub fn new(plugin: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            action: action.into(),
        }
    }
}

/// The faults the stored runs injected: the fault ids of the graph's
/// `injects` edges. A run ingested with its definition keys the fault of a
/// plugin function by [`plugin_fault_id`]; a run ingested without one keys
/// its faults by activity name, which no catalog action matches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestedFaults(HashSet<String>);

impl TestedFaults {
    /// The set of the fault ids `ids`, as the `injects` edges name them.
    pub fn from_fault_ids<I, S>(ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self(ids.into_iter().map(Into::into).collect())
    }

    /// Whether a stored run injected the fault `action` names: the function
    /// `action.action` of the plugin `action.plugin`.
    #[must_use]
    pub fn contains(&self, action: &AvailableAction) -> bool {
        self.0
            .contains(&plugin_fault_id(&action.plugin, &action.action))
    }
}

/// Sentinel `run_id` under which coverage-gap edges are stored, so the whole
/// coverage-gap sub-graph can be cleared and re-derived on demand.
pub const COVERAGE_GAP_RUN_ID: &str = "__coverage_gaps__";

/// Derive the coverage-gap sub-graph: a [`NodeKind::CoverageGap`] node per
/// untested action, its owning [`NodeKind::FaultDomain`] node, and a `gap_in`
/// edge between them. Deterministic and deduplicated.
#[must_use]
pub fn coverage_gap_delta(available: &[AvailableAction], tested: &TestedFaults) -> GraphDelta {
    let mut delta = GraphDelta::default();
    let mut seen_nodes = HashSet::new();
    let mut seen_edges = HashSet::new();

    for action in available {
        if tested.contains(action) {
            continue;
        }
        let gap_id = format!("gap:{}::{}", action.plugin, action.action);
        let domain_id = format!("domain:{}", action.plugin);

        if seen_nodes.insert(domain_id.clone()) {
            delta.nodes.push(Node {
                id: domain_id.clone(),
                kind: NodeKind::FaultDomain,
                label: action.plugin.clone(),
                attrs: serde_json::json!({ "plugin": action.plugin }),
            });
        }
        if seen_nodes.insert(gap_id.clone()) {
            delta.nodes.push(Node {
                id: gap_id.clone(),
                kind: NodeKind::CoverageGap,
                label: format!("{}::{}", action.plugin, action.action),
                attrs: serde_json::json!({
                    "plugin": action.plugin,
                    "action": action.action,
                }),
            });
        }
        if seen_edges.insert((gap_id.clone(), domain_id.clone())) {
            delta.edges.push(Edge {
                src: gap_id,
                rel: EdgeRel::GapIn,
                dst: domain_id,
                attrs: serde_json::json!({}),
            });
        }
    }
    delta
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untested_actions_become_gaps_with_domain_edges() {
        let available = [
            AvailableAction::new("tumult-net", "inject_latency"),
            AvailableAction::new("tumult-net", "drop_packets"),
            AvailableAction::new("tumult-ssh", "execute"),
        ];
        let tested = TestedFaults::from_fault_ids(["fault:tumult-net::inject_latency"]);

        let delta = coverage_gap_delta(&available, &tested);

        let ids: Vec<&str> = delta.nodes.iter().map(|n| n.id.as_str()).collect();
        // Tested action is not a gap.
        assert!(!ids.contains(&"gap:tumult-net::inject_latency"));
        // Untested actions are gaps.
        assert!(ids.contains(&"gap:tumult-net::drop_packets"));
        assert!(ids.contains(&"gap:tumult-ssh::execute"));
        // Domain nodes present, deduplicated (one tumult-net domain).
        assert!(ids.contains(&"domain:tumult-net"));
        assert!(ids.contains(&"domain:tumult-ssh"));
        assert_eq!(
            ids.iter().filter(|id| **id == "domain:tumult-net").count(),
            1
        );

        // gap_in edges connect each gap to its domain.
        assert!(delta
            .edges
            .iter()
            .any(|e| e.src == "gap:tumult-net::drop_packets"
                && e.rel == EdgeRel::GapIn
                && e.dst == "domain:tumult-net"));
    }

    #[test]
    fn all_tested_yields_empty_delta() {
        let available = [AvailableAction::new("tumult-net", "inject_latency")];
        let tested = TestedFaults::from_fault_ids(["fault:tumult-net::inject_latency"]);
        let delta = coverage_gap_delta(&available, &tested);
        assert!(delta.nodes.is_empty());
        assert!(delta.edges.is_empty());
    }

    #[test]
    fn duplicate_catalog_entries_are_deduplicated() {
        let available = [
            AvailableAction::new("tumult-net", "drop_packets"),
            AvailableAction::new("tumult-net", "drop_packets"),
            AvailableAction::new("tumult-net", "inject_latency"),
        ];
        let tested = TestedFaults::default();

        let delta = coverage_gap_delta(&available, &tested);

        let gap_ids: Vec<&str> = delta
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::CoverageGap)
            .map(|n| n.id.as_str())
            .collect();
        assert_eq!(
            gap_ids,
            vec![
                "gap:tumult-net::drop_packets",
                "gap:tumult-net::inject_latency"
            ],
            "each untested action appears exactly once"
        );
        assert_eq!(delta.edges.len(), 2, "no duplicate gap_in edges");
    }
}
