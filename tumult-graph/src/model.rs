//! The graph model: node kinds, edge relations, and the value types used to
//! persist a run ([`GraphDelta`]) and to return query results
//! ([`NodeSummary`], [`EgoGraph`]).

use serde::{Deserialize, Serialize};

/// The kind of a graph node.
///
/// A chaos run is modelled with five per-run/identity node kinds. `Experiment`
/// and `Service` are stable identities that recur across runs; `Journal` and
/// `Deviation` are per-run; `Fault` is shared by every run that injects the
/// same fault. Phase 2 adds three deterministic, mostly-static kinds:
/// `ComplianceArticle` (from the compliance citation registry), `CoverageGap`
/// (an untested plugin action) and `FaultDomain` (a plugin grouping targeted
/// by coverage gaps).
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
    /// A regulatory control/article from the compliance citation registry
    /// (e.g. `compliance:DORA/Art.25`). Static and deterministic.
    ComplianceArticle,
    /// An untested plugin action (`gap:<plugin>::<action>`): a fault primitive
    /// available in the catalog that has never appeared in a tested run.
    CoverageGap,
    /// A plugin grouping (`domain:<plugin>`) that coverage gaps belong to.
    FaultDomain,
    /// An autopilot decision record (`rec:<uuid>`): why an autonomous run
    /// was — or was not — enacted. The full decision detail lives in the
    /// decision store; the node carries verdict/score/policy-hash attrs so
    /// lineage is queryable in-graph.
    Recommendation,
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
            Self::ComplianceArticle => "compliance_article",
            Self::CoverageGap => "coverage_gap",
            Self::FaultDomain => "fault_domain",
            Self::Recommendation => "recommendation",
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
            "compliance_article" => Some(Self::ComplianceArticle),
            "coverage_gap" => Some(Self::CoverageGap),
            "fault_domain" => Some(Self::FaultDomain),
            "recommendation" => Some(Self::Recommendation),
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
    /// `Experiment -> ComplianceArticle`: a passing run supplies evidence
    /// toward this control. Carries the citation `strength` on the edge attrs.
    Evidences,
    /// `Experiment -> ComplianceArticle`: the experiment definition *declares*
    /// a mapping to this control (intent, not run-produced evidence).
    MapsToCompliance,
    /// `CoverageGap -> FaultDomain`: this untested action is a gap in the
    /// plugin/fault-domain.
    GapIn,
    /// `Service -> Service`: declared runtime dependency (src depends on
    /// dst), imported from a topology document — never derived from runs.
    DependsOn,
    /// `Deviation -> Fault`: the injected fault attributed as the cause of
    /// this deviation (only emitted when attribution is unambiguous).
    CausedBy,
    /// `Recommendation -> Journal`: the autopilot decision that enacted
    /// this run. Proposed/vetoed decisions have no run to point at — their
    /// full record lives on the recommendation node and in the decision
    /// store.
    Enacted,
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
            Self::Evidences => "evidences",
            Self::MapsToCompliance => "maps_to_compliance",
            Self::GapIn => "gap_in",
            Self::DependsOn => "depends_on",
            Self::CausedBy => "caused_by",
            Self::Enacted => "enacted",
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
            "evidences" => Some(Self::Evidences),
            "maps_to_compliance" => Some(Self::MapsToCompliance),
            "gap_in" => Some(Self::GapIn),
            "depends_on" => Some(Self::DependsOn),
            "caused_by" => Some(Self::CausedBy),
            "enacted" => Some(Self::Enacted),
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
    /// Small structured attributes, stored as JSON (e.g. the citation
    /// `strength` on an `evidences` edge). Empty object when unused.
    #[serde(default)]
    pub attrs: serde_json::Value,
}

/// The set of nodes and edges a single run contributes to the graph.
///
/// The delta is deduplicated: mapping the same journal twice yields the same
/// nodes/edges with no duplicates, so persistence is idempotent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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

/// One full edge row read back from storage — the raw material for lineage
/// and recommendation computation (which stay pure by consuming these).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EdgeRecord {
    pub src: String,
    pub rel: String,
    pub dst: String,
    pub run_id: String,
    pub ts: i64,
    /// Edge attrs as JSON text (`{}` when absent).
    pub attrs: String,
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
            NodeKind::ComplianceArticle,
            NodeKind::CoverageGap,
            NodeKind::FaultDomain,
            NodeKind::Recommendation,
        ] {
            assert_eq!(NodeKind::parse(kind.as_str()), Some(kind));
            assert_eq!(NodeKind::parse(&kind.as_str().to_uppercase()), Some(kind));
        }
        assert_eq!(NodeKind::parse("nope"), None);
    }

    #[test]
    fn node_kind_display_matches_canonical_token() {
        assert_eq!(NodeKind::Experiment.to_string(), "experiment");
        assert_eq!(NodeKind::FaultDomain.to_string(), "fault_domain");
        assert_eq!(NodeKind::Recommendation.to_string(), "recommendation");
    }

    #[test]
    fn edge_rel_round_trips_through_token() {
        for rel in [
            EdgeRel::Targets,
            EdgeRel::Injects,
            EdgeRel::Yielded,
            EdgeRel::ObservedOn,
            EdgeRel::Exhibited,
            EdgeRel::Evidences,
            EdgeRel::MapsToCompliance,
            EdgeRel::GapIn,
            EdgeRel::DependsOn,
            EdgeRel::CausedBy,
            EdgeRel::Enacted,
        ] {
            assert_eq!(EdgeRel::parse(rel.as_str()), Some(rel));
        }
        assert_eq!(EdgeRel::parse("nope"), None);
    }

    #[test]
    fn edge_rel_display_matches_canonical_token() {
        assert_eq!(EdgeRel::Targets.to_string(), "targets");
        assert_eq!(EdgeRel::ObservedOn.to_string(), "observed_on");
        assert_eq!(EdgeRel::CausedBy.to_string(), "caused_by");
    }
}
