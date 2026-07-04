//! The graph model: node kinds, edge relations, and the value types used to
//! persist a run ([`GraphDelta`]) and to return query results
//! ([`NodeSummary`], [`EgoGraph`]).

use serde::{Deserialize, Serialize};

/// The kind of a graph node.
///
/// A chaos run is modelled with five node kinds. `Experiment` and `Service`
/// are stable identities that recur across runs; `Journal` and `Deviation`
/// are per-run; `Fault` is shared by every run that injects the same fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    /// A named experiment (by title) — the stable system-under-test identity.
    Experiment,
    /// A fault primitive, `plugin::function` (e.g. `tumult-net::inject_latency`)
    /// or, when only a journal is available, the injecting activity's name.
    Fault,
    /// A service/target the experiment acts on (e.g. `demo-app`).
    Service,
    /// A single experiment run's journal, labelled with its terminal status.
    Journal,
    /// A run's deviation: present when the run did not complete cleanly.
    Deviation,
}

impl NodeKind {
    /// The canonical lowercase token stored in the `kind` column.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Experiment => "experiment",
            Self::Fault => "fault",
            Self::Service => "service",
            Self::Journal => "journal",
            Self::Deviation => "deviation",
        }
    }

    /// Parse a `kind` token back into a [`NodeKind`]. Case-insensitive.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "experiment" => Some(Self::Experiment),
            "fault" => Some(Self::Fault),
            "service" => Some(Self::Service),
            "journal" => Some(Self::Journal),
            "deviation" => Some(Self::Deviation),
            _ => None,
        }
    }
}

impl std::fmt::Display for NodeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The relation carried by a graph edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeRel {
    /// `Experiment -> Service`: the experiment acts on this service.
    Targets,
    /// `Experiment -> Fault`: the experiment injects this fault.
    Injects,
    /// `Experiment -> Journal`: this run of the experiment produced this journal.
    Yielded,
    /// `Fault -> Service`: this fault was injected on this service.
    ObservedOn,
    /// `Journal -> Deviation`: this run exhibited this deviation.
    Exhibited,
}

impl EdgeRel {
    /// The canonical lowercase token stored in the `rel` column.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Targets => "targets",
            Self::Injects => "injects",
            Self::Yielded => "yielded",
            Self::ObservedOn => "observed_on",
            Self::Exhibited => "exhibited",
        }
    }

    /// Parse a `rel` token back into an [`EdgeRel`]. Case-insensitive.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "targets" => Some(Self::Targets),
            "injects" => Some(Self::Injects),
            "yielded" => Some(Self::Yielded),
            "observed_on" => Some(Self::ObservedOn),
            "exhibited" => Some(Self::Exhibited),
            _ => None,
        }
    }
}

impl std::fmt::Display for EdgeRel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A graph node, keyed by a stable string [`id`](Node::id).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// Stable, deterministic identifier (e.g. `exp:<title>`, `fault:<p>::<f>`).
    pub id: String,
    /// The node kind.
    pub kind: NodeKind,
    /// A short human-readable label (e.g. the title, `plugin::function`).
    pub label: String,
    /// Small structured attributes, stored as JSON.
    pub attrs: serde_json::Value,
}

/// A directed graph edge produced by a single run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    /// Source node id.
    pub src: String,
    /// Edge relation.
    pub rel: EdgeRel,
    /// Destination node id.
    pub dst: String,
}

/// The set of nodes and edges a single run contributes to the graph.
///
/// The delta is deduplicated: mapping the same journal twice yields the same
/// nodes/edges with no duplicates, so persistence is idempotent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GraphDelta {
    /// Nodes to upsert (deduplicated by id).
    pub nodes: Vec<Node>,
    /// Edges to record (deduplicated by `(src, rel, dst)`).
    pub edges: Vec<Edge>,
}

/// A one-line node summary returned by node/kind queries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeSummary {
    /// Node id.
    pub id: String,
    /// Node kind token.
    pub kind: String,
    /// Node label.
    pub label: String,
}

/// A compact `(src)-[rel]->(dst)` tuple in an ego sub-graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EgoTuple {
    /// Source node id.
    pub src: String,
    /// Edge relation token.
    pub rel: String,
    /// Destination node id.
    pub dst: String,
}

/// The neighbourhood ("ego sub-graph") of a node: the nodes reachable within
/// the requested depth plus the edges among them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EgoGraph {
    /// The node the neighbourhood is centred on.
    pub center: String,
    /// Every node appearing in the neighbourhood (including the centre).
    pub nodes: Vec<NodeSummary>,
    /// The `(src)-[rel]->(dst)` tuples among those nodes.
    pub edges: Vec<EgoTuple>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_kind_round_trips_through_token() {
        for kind in [
            NodeKind::Experiment,
            NodeKind::Fault,
            NodeKind::Service,
            NodeKind::Journal,
            NodeKind::Deviation,
        ] {
            assert_eq!(NodeKind::parse(kind.as_str()), Some(kind));
            assert_eq!(NodeKind::parse(&kind.as_str().to_uppercase()), Some(kind));
        }
        assert_eq!(NodeKind::parse("nope"), None);
    }

    #[test]
    fn edge_rel_round_trips_through_token() {
        for rel in [
            EdgeRel::Targets,
            EdgeRel::Injects,
            EdgeRel::Yielded,
            EdgeRel::ObservedOn,
            EdgeRel::Exhibited,
        ] {
            assert_eq!(EdgeRel::parse(rel.as_str()), Some(rel));
        }
        assert_eq!(EdgeRel::parse("nope"), None);
    }
}
