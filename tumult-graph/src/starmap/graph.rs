//! The working graph: nodes and edges keyed so iteration order, and with
//! it the serialised output, never depends on input order.

use std::collections::{BTreeMap, BTreeSet};

use super::model::{EdgeSource, StarEdge, StarKind, StarNode, StarRel};

pub(super) type EdgeKey = (String, StarRel, String);

#[derive(Debug, Default)]
pub(super) struct Graph {
    pub nodes: BTreeMap<String, StarNode>,
    pub edges: BTreeMap<EdgeKey, StarEdge>,
}

impl Graph {
    /// Get or create a node. `declared` says whether this sighting is a
    /// declaration only; one observed sighting clears the flag for good.
    pub fn ensure(
        &mut self,
        id: &str,
        kind: StarKind,
        label: &str,
        declared: bool,
    ) -> &mut StarNode {
        let node = self.nodes.entry(id.to_string()).or_insert_with(|| {
            let mut fresh = StarNode::new(id, kind, label);
            fresh.declared = true;
            fresh
        });
        node.declared &= declared;
        node
    }

    /// Add an edge, or keep the existing one with the stronger source.
    pub fn link(
        &mut self,
        from: &str,
        rel: StarRel,
        to: &str,
        source: EdgeSource,
    ) -> &mut StarEdge {
        let key = (from.to_string(), rel, to.to_string());
        let edge = self.edges.entry(key).or_insert_with(|| StarEdge {
            id: format!("{from}|{}|{to}", rel.as_str()),
            from: from.to_string(),
            to: to.to_string(),
            rel,
            source,
            weight: None,
            errors: None,
            avg_duration_ns: None,
        });
        edge.source = edge.source.min(source);
        edge
    }

    pub fn has(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    /// Keep only the nodes in `keep`, then drop edges that lost an end.
    pub fn retain(&mut self, keep: &BTreeSet<String>) {
        self.nodes.retain(|id, _| keep.contains(id));
        self.drop_dangling();
    }

    pub fn drop_dangling(&mut self) {
        let nodes = &self.nodes;
        self.edges
            .retain(|(from, _, to), _| nodes.contains_key(from) && nodes.contains_key(to));
    }

    /// Undirected adjacency over typed edges and spine links.
    pub fn adjacency(&self) -> BTreeMap<&str, BTreeSet<&str>> {
        let mut adjacent: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for (from, _, to) in self.edges.keys() {
            adjacent.entry(from).or_default().insert(to);
            adjacent.entry(to).or_default().insert(from);
        }
        for (id, node) in &self.nodes {
            let Some(parent) = node.parent.as_deref() else {
                continue;
            };
            if !self.nodes.contains_key(parent) {
                continue;
            }
            adjacent.entry(id).or_default().insert(parent);
            adjacent.entry(parent).or_default().insert(id);
        }
        adjacent
    }
}
