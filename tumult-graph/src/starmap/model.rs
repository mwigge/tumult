//! The Star Map response: one node list, one edge list, and the overlay
//! blocks every lens reads from.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::{EdgeRel, NodeKind};

/// Node kinds the Star Map exposes. Each maps onto one `ChaosGraph`
/// [`NodeKind`], so both graphs share a vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StarKind {
    Org,
    Entity,
    System,
    Component,
    Service,
    Workload,
    Ext,
    Def,
    Run,
    Slo,
    Article,
    Gap,
    Cluster,
}

impl StarKind {
    /// Every kind, in ring order.
    pub const ALL: [Self; 13] = [
        Self::Org,
        Self::Entity,
        Self::System,
        Self::Component,
        Self::Service,
        Self::Workload,
        Self::Ext,
        Self::Def,
        Self::Run,
        Self::Slo,
        Self::Article,
        Self::Gap,
        Self::Cluster,
    ];

    /// Wire token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Org => "org",
            Self::Entity => "entity",
            Self::System => "system",
            Self::Component => "component",
            Self::Service => "service",
            Self::Workload => "workload",
            Self::Ext => "ext",
            Self::Def => "def",
            Self::Run => "run",
            Self::Slo => "slo",
            Self::Article => "article",
            Self::Gap => "gap",
            Self::Cluster => "cluster",
        }
    }

    /// Parse a wire token.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == token)
    }

    /// The `ChaosGraph` kind this node would be stored as. A cluster is a
    /// rendering aggregate and has none.
    #[must_use]
    pub fn graph_kind(self) -> Option<NodeKind> {
        match self {
            Self::Org => Some(NodeKind::OrgUnit),
            Self::Entity => Some(NodeKind::IctEntity),
            Self::System => Some(NodeKind::System),
            Self::Component => Some(NodeKind::Component),
            Self::Service => Some(NodeKind::Service),
            Self::Workload => Some(NodeKind::Workload),
            Self::Ext => Some(NodeKind::External),
            Self::Def => Some(NodeKind::Experiment),
            Self::Run => Some(NodeKind::Journal),
            Self::Slo => Some(NodeKind::Slo),
            Self::Article => Some(NodeKind::ComplianceArticle),
            Self::Gap => Some(NodeKind::CoverageGap),
            Self::Cluster => None,
        }
    }

    /// Layout ring: 1 org, 2 entity, 3 system and component, 4 service,
    /// 5 workload and external, 6 satellites. Articles and clusters are
    /// never collapsed (`0`).
    #[must_use]
    pub fn ring(self) -> u8 {
        match self {
            Self::Article | Self::Cluster => 0,
            Self::Org => 1,
            Self::Entity => 2,
            Self::System | Self::Component => 3,
            Self::Service => 4,
            Self::Workload | Self::Ext => 5,
            Self::Def | Self::Run | Self::Slo | Self::Gap => 6,
        }
    }
}

/// The nine relations the Star Map exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StarRel {
    Owns,
    Implements,
    Calls,
    RunsOn,
    DependsOn,
    TestedBy,
    Broke,
    Covers,
    Measures,
}

impl StarRel {
    /// Wire token.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owns => "owns",
            Self::Implements => "implements",
            Self::Calls => "calls",
            Self::RunsOn => "runs_on",
            Self::DependsOn => "depends_on",
            Self::TestedBy => "tested_by",
            Self::Broke => "broke",
            Self::Covers => "covers",
            Self::Measures => "measures",
        }
    }

    /// The one place `ChaosGraph` relation names map to Star Map names.
    /// Relations with no Star Map meaning return `None`.
    #[must_use]
    pub fn from_graph(rel: EdgeRel) -> Option<Self> {
        match rel {
            EdgeRel::Owns => Some(Self::Owns),
            EdgeRel::Implements => Some(Self::Implements),
            EdgeRel::RunsOn => Some(Self::RunsOn),
            EdgeRel::Measures => Some(Self::Measures),
            EdgeRel::DependsOn => Some(Self::DependsOn),
            EdgeRel::Targets | EdgeRel::Yielded => Some(Self::TestedBy),
            EdgeRel::Evidences | EdgeRel::MapsToCompliance => Some(Self::Covers),
            EdgeRel::Exhibited | EdgeRel::ObservedOn => Some(Self::Broke),
            _ => None,
        }
    }
}

/// Where an edge was learned from. When two sources report the same edge
/// the earlier variant wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeSource {
    /// Resource attributes on telemetry.
    Telemetry,
    /// Parent and child spans.
    Observed,
    /// Runs and their outcomes.
    Evidence,
    /// `org.yaml`, manifests or a topology file.
    Declared,
}

/// Evidence state of an in-scope node. Ordered so the worse state is
/// the greater one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Gap {
    /// Evidence exists but none of it is fresh.
    Stale,
    /// No evidence in any discipline.
    Missing,
}

/// Something about a node the reader should not trust at face value.
/// Serialised as a sorted list of tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Flag {
    /// The entity id does not have the agreed form
    /// (`^[a-z][a-z0-9]*(-[a-z0-9]+)+$`, at most 63 characters). It is
    /// shown exactly as reported, never rewritten.
    MalformedId,
    /// Telemetry reports more than one owner for the entity.
    MultipleOwners,
    /// Telemetry and `org.yaml` name different owners for the entity.
    OwnerConflict,
}

/// What hangs below an entity or an org unit, counted. The `gap` of an
/// entity is the best of its systems, so one tested system clears it;
/// these counts say how many systems that hides.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Rollup {
    pub systems: usize,
    /// Systems with no evidence in any discipline.
    pub untested_systems: usize,
    /// Systems whose evidence is all stale.
    pub stale_systems: usize,
    /// Failed runs against those systems.
    pub failed: u64,
    pub risks_open: u64,
    pub risks_suppressed: u64,
    /// Nodes below with a gap: for an entity its systems, for an org
    /// unit everything on its spine except entities.
    pub gaps: usize,
}

/// Posture block, copied from the posture computation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Posture {
    pub score: f64,
    pub band: String,
}

/// Counts only; the drawer loads the rows from existing endpoints.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Evidence {
    pub runs: u64,
    pub failed: u64,
    pub manual: u64,
    pub last_run_ns: Option<i64>,
    /// Discipline to `covered`, `stale` or `missing`.
    pub by_discipline: BTreeMap<String, String>,
}

/// Risks attached to a node. Suppressed is never the same as absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RiskSummary {
    pub open: u64,
    pub suppressed: u64,
    /// Worst severity among open risks.
    pub worst: Option<String>,
    /// Earliest expiry among the suppressions.
    pub suppression_expires_ns: Option<i64>,
    pub suppressed_by: Option<String>,
    pub suppression_reason: Option<String>,
    /// Disciplines the open risks recommend testing.
    pub recommended: Vec<String>,
}

/// One node. Every field is always present so the shape is stable; the
/// `slo`, `dora` and `maturity` blocks are reserved for the score
/// document's sources and stay `null` until those exist.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StarNode {
    pub id: String,
    pub kind: StarKind,
    pub label: String,
    /// Spine parent used by the layout.
    pub parent: Option<String>,
    pub env: Option<String>,
    pub tier: Option<String>,
    /// Org-tree kind for `org` nodes (`company`, `team`, ...).
    pub org_kind: Option<String>,
    pub criticality_weight: f64,
    pub conformance: Option<String>,
    pub last_seen_ns: Option<i64>,
    pub posture: Option<Posture>,
    pub evidence: Evidence,
    pub gap: Option<Gap>,
    pub risks: RiskSummary,
    pub slo: Option<serde_json::Value>,
    pub slo_count: u32,
    pub dora: Option<serde_json::Value>,
    /// Reserved: a per-node maturity score with its layer roll-up.
    pub maturity: Option<serde_json::Value>,
    /// Two different entity ids were reported for this service.
    pub conflict: bool,
    /// Warnings about the node itself, sorted.
    pub flags: Vec<Flag>,
    /// Counts over what hangs below; entities and org units only.
    pub rollup: Option<Rollup>,
    /// Known only from a declaration (manifest or topology file).
    pub declared: bool,
    /// Number of nodes whose spine parent is this node.
    pub children: usize,
    /// Collapsed node count, for clusters.
    pub members: Option<usize>,
}

impl StarNode {
    /// A node with empty overlays.
    #[must_use]
    pub fn new(id: impl Into<String>, kind: StarKind, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind,
            label: label.into(),
            parent: None,
            env: None,
            tier: None,
            org_kind: None,
            criticality_weight: 1.0,
            conformance: None,
            last_seen_ns: None,
            posture: None,
            evidence: Evidence::default(),
            gap: None,
            risks: RiskSummary::default(),
            slo: None,
            slo_count: 0,
            dora: None,
            maturity: None,
            conflict: false,
            flags: Vec::new(),
            rollup: None,
            declared: false,
            children: 0,
            members: None,
        }
    }
}

/// One edge. The id is `from|rel|to`, so it is stable across requests.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StarEdge {
    pub id: String,
    pub from: String,
    pub to: String,
    pub rel: StarRel,
    pub source: EdgeSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weight: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avg_duration_ns: Option<f64>,
}

/// Worst overlay state inside a cluster.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ClusterWorst {
    pub gap: Option<Gap>,
    pub risk: Option<String>,
}

/// Summary of one collapsed group; the matching node has kind `cluster`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StarCluster {
    pub id: String,
    pub label: String,
    pub members: usize,
    pub worst: ClusterWorst,
}

/// Totals for the legend.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Counts {
    pub nodes: usize,
    pub edges: usize,
    /// Nodes removed by `focus` and `kinds`.
    pub pruned: usize,
    /// Nodes collapsed into clusters.
    pub clustered: usize,
    /// Risks and gaps are counted before clustering, so they do not
    /// change with the node budget.
    pub risks_open: u64,
    pub risks_suppressed: u64,
    /// Risks the caller may see that belong to no node on the map.
    pub risks_unplaced: u64,
    pub gaps: usize,
}

/// The whole response.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StarMap {
    pub as_of_ns: i64,
    pub range: String,
    pub env: Option<String>,
    pub focus: Option<String>,
    /// With a `focus`: whether that node exists and the caller may see
    /// it. `false` comes with an empty map.
    pub focus_found: Option<bool>,
    pub depth: Option<u8>,
    pub disclaimer: &'static str,
    /// A source table was longer than its row limit: evidence edges,
    /// articles, gaps or declared dependencies may be missing.
    pub truncated: bool,
    pub maturity: Option<serde_json::Value>,
    pub counts: Counts,
    pub nodes: Vec<StarNode>,
    pub edges: Vec<StarEdge>,
    pub clusters: Vec<StarCluster>,
}
