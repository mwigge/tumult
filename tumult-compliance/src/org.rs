//! Org hierarchy rollups: a YAML-driven, single-parent tree of arbitrary
//! depth (Backstage-style `org.yaml`) that aggregates per-experiment
//! resilience scores into team/unit/domain/company rollups.
//!
//! Model rules:
//!
//! * The tree has an implicit company root (path `""`); every declared node
//!   has at most one parent and node names are unique across the file.
//! * Experiments attach to nodes via `assignments` (`team` = node name,
//!   `targets` = name globs). Only `*` is supported in globs. The first
//!   matching assignment wins; unmapped experiments land in a visible
//!   synthetic `(unassigned)` node directly under the root.
//! * Criticality per experiment name: `critical` weighs 3, `high` weighs 2,
//!   everything else weighs 1 (multiplied by `defaults.weight`).
//! * **Node score = criticality-weighted mean over ALL leaves in the
//!   subtree**, recomputed from the leaves — never an average of child
//!   means (a one-experiment team must not pull a domain as much as a
//!   ten-experiment team). Companion coverage = scored / expected leaves.
//! * Pending manual records (draft/submitted) and excluded outcomes
//!   (inconclusive) count toward `expected` but carry no score weight.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use serde::Deserialize;

/// Name of the synthetic node holding unmapped experiments.
pub const UNASSIGNED: &str = "(unassigned)";

/// Errors loading or validating an org file.
#[derive(Debug)]
pub enum OrgError {
    Io(std::io::Error),
    Yaml(String),
    Invalid(String),
}

impl fmt::Display for OrgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cannot read org file: {e}"),
            Self::Yaml(e) => write!(f, "cannot parse org yaml: {e}"),
            Self::Invalid(e) => write!(f, "invalid org file: {e}"),
        }
    }
}

impl std::error::Error for OrgError {}

/// Raw YAML shape.
#[derive(Debug, Deserialize)]
struct OrgFile {
    #[serde(default)]
    nodes: Vec<NodeSpec>,
    #[serde(default)]
    assignments: Vec<AssignmentSpec>,
    #[serde(default)]
    defaults: DefaultsSpec,
}

#[derive(Debug, Deserialize)]
struct NodeSpec {
    name: String,
    #[serde(default = "default_kind")]
    kind: String,
    parent: Option<String>,
}

fn default_kind() -> String {
    "team".into()
}

#[derive(Debug, Deserialize)]
struct AssignmentSpec {
    team: String,
    #[serde(default)]
    targets: Vec<String>,
    #[serde(default)]
    criticality: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
struct DefaultsSpec {
    #[serde(default = "default_weight")]
    weight: f64,
    #[serde(default = "default_stale_days")]
    stale_days: u32,
}

impl Default for DefaultsSpec {
    fn default() -> Self {
        Self {
            weight: default_weight(),
            stale_days: default_stale_days(),
        }
    }
}

const fn default_weight() -> f64 {
    1.0
}

const fn default_stale_days() -> u32 {
    30
}

/// One node in the resolved tree (arena index).
#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub kind: String,
    parent: Option<usize>,
    children: Vec<usize>,
}

/// A compiled assignment: globs + per-name criticality for one node.
#[derive(Debug, Clone)]
struct Assignment {
    node: usize,
    targets: Vec<String>,
    criticality: BTreeMap<String, String>,
}

/// The resolved org tree. Cheap to clone into shared state.
#[derive(Debug, Clone)]
pub struct OrgTree {
    nodes: Vec<Node>,
    root: usize,
    unassigned: usize,
    by_name: HashMap<String, usize>,
    assignments: Vec<Assignment>,
    default_weight: f64,
    stale_days: u32,
}

/// Criticality weight: `critical` = 3, `high` = 2, default 1.
#[must_use]
pub fn criticality_weight(level: &str) -> u32 {
    match level {
        "critical" => 3,
        "high" => 2,
        _ => 1,
    }
}

/// Minimal glob: `*` matches any (possibly empty) character sequence;
/// everything else is literal. Anchored at both ends unless the pattern
/// starts/ends with `*`.
#[must_use]
pub fn glob_match(pattern: &str, name: &str) -> bool {
    let anchored_start = !pattern.starts_with('*');
    let anchored_end = !pattern.ends_with('*');
    let segments: Vec<&str> = pattern.split('*').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return true; // pattern is all '*'
    }
    let mut rest = name;
    for (i, seg) in segments.iter().enumerate() {
        if i == 0 && anchored_start {
            let Some(stripped) = rest.strip_prefix(seg) else {
                return false;
            };
            rest = stripped;
            continue;
        }
        let Some(pos) = rest.find(seg) else {
            return false;
        };
        rest = &rest[pos + seg.len()..];
    }
    if anchored_end {
        // The last segment must reach the very end of the name.
        name.ends_with(segments.last().unwrap_or(&""))
    } else {
        true
    }
}

/// One experiment presented to the rollup: its name and either a score or a
/// reason it carries no score weight.
#[derive(Debug, Clone)]
pub struct ScoredLeaf {
    pub name: String,
    /// `Some(score)` for score-contributing leaves (automated runs and
    /// verified manual records, excluding inconclusive outcomes);
    /// `None` for leaves that only count toward `expected`.
    pub score: Option<u32>,
}

/// Rollup for one node: weighted mean over every leaf in its subtree.
#[derive(Debug, Clone, serde::Serialize)]
pub struct OrgNodeScore {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub score: f64,
    pub band: String,
    /// scored / expected (0.0 when no leaves are mapped into the subtree).
    pub coverage: f64,
    pub scored: usize,
    pub expected: usize,
    /// Name of the lowest-scoring scored leaf in the subtree.
    pub weakest: Option<String>,
    /// Σ criticality weights over all leaves (the treemap area).
    pub weight: f64,
    /// Direct children, one level down, weakest first.
    pub children: Vec<OrgNodeScore>,
}

impl OrgTree {
    /// The trivial tree: implicit root + `(unassigned)`, no assignments.
    /// Used when no org file is configured.
    #[must_use]
    pub fn empty() -> Self {
        let nodes = vec![
            Node {
                name: String::new(),
                kind: "company".into(),
                parent: None,
                children: vec![1],
            },
            Node {
                name: UNASSIGNED.into(),
                kind: "unassigned".into(),
                parent: Some(0),
                children: vec![],
            },
        ];
        Self {
            nodes,
            root: 0,
            unassigned: 1,
            by_name: HashMap::from([(UNASSIGNED.to_string(), 1)]),
            assignments: vec![],
            default_weight: 1.0,
            stale_days: 30,
        }
    }

    /// Load and validate an org file from disk.
    ///
    /// # Errors
    /// Returns `Io`/`Yaml`/`Invalid` when the file cannot be read, parsed or
    /// fails validation.
    pub fn load(path: &std::path::Path) -> Result<Self, OrgError> {
        let text = std::fs::read_to_string(path).map_err(OrgError::Io)?;
        Self::from_yaml(&text)
    }

    /// Parse and validate from a YAML string.
    ///
    /// # Errors
    /// Returns `Yaml`/`Invalid` on parse or validation failure.
    pub fn from_yaml(text: &str) -> Result<Self, OrgError> {
        let file: OrgFile =
            serde_yaml::from_str(text).map_err(|e| OrgError::Yaml(e.to_string()))?;
        Self::build(file)
    }

    fn build(file: OrgFile) -> Result<Self, OrgError> {
        let mut tree = Self::empty();
        tree.default_weight = file.defaults.weight;
        tree.stale_days = file.defaults.stale_days;

        // Uniqueness and name validity.
        let mut seen = HashSet::new();
        for spec in &file.nodes {
            if spec.name.trim().is_empty()
                || spec.name.contains('/')
                || spec.name == UNASSIGNED
                || !seen.insert(spec.name.clone())
            {
                return Err(OrgError::Invalid(format!(
                    "duplicate or invalid node name '{}'",
                    spec.name
                )));
            }
        }

        // Insert nodes (parent resolved by unique name).
        for spec in &file.nodes {
            let parent = match &spec.parent {
                None => tree.root,
                Some(p) => *tree.by_name.get(p).ok_or_else(|| {
                    OrgError::Invalid(format!("node '{}' has unknown parent '{p}'", spec.name))
                })?,
            };
            let id = tree.nodes.len();
            tree.nodes.push(Node {
                name: spec.name.clone(),
                kind: spec.kind.clone(),
                parent: Some(parent),
                children: vec![],
            });
            tree.nodes[parent].children.push(id);
            tree.by_name.insert(spec.name.clone(), id);
        }

        // Cycle check: walking parents from any node must reach the root.
        // (Parents are inserted before children only when the parent was
        // declared earlier, so an ordering-independent check walks the
        // parent chain with a visited set.)
        for id in 0..tree.nodes.len() {
            let mut visited = HashSet::new();
            let mut cur = Some(id);
            while let Some(n) = cur {
                if !visited.insert(n) {
                    return Err(OrgError::Invalid(format!(
                        "cycle detected involving node '{}'",
                        tree.nodes[n].name
                    )));
                }
                cur = tree.nodes[n].parent;
            }
        }

        // Compile assignments.
        for spec in file.assignments {
            let node = *tree.by_name.get(&spec.team).ok_or_else(|| {
                OrgError::Invalid(format!("assignment targets unknown team '{}'", spec.team))
            })?;
            tree.assignments.push(Assignment {
                node,
                targets: spec.targets,
                criticality: spec.criticality,
            });
        }
        Ok(tree)
    }

    /// Resolve a slash path (`""` = root, `"platform/edge"`) to a node.
    #[must_use]
    pub fn resolve(&self, path: &str) -> Option<usize> {
        let trimmed = path.trim_matches('/');
        if trimmed.is_empty() {
            return Some(self.root);
        }
        let mut cur = self.root;
        for segment in trimmed.split('/') {
            let next = self.nodes[cur]
                .children
                .iter()
                .find(|&&c| self.nodes[c].name == segment)?;
            cur = *next;
        }
        Some(cur)
    }

    /// The slash path of a node (`""` for the root).
    #[must_use]
    pub fn path_of(&self, mut id: usize) -> String {
        let mut parts = vec![];
        while let Some(parent) = self.nodes[id].parent {
            parts.push(self.nodes[id].name.as_str());
            id = parent;
        }
        parts.reverse();
        parts.join("/")
    }

    /// Direct children of a node id.
    #[must_use]
    pub fn children(&self, id: usize) -> &[usize] {
        &self.nodes[id].children
    }

    /// A node by arena id.
    #[must_use]
    pub fn node(&self, id: usize) -> &Node {
        &self.nodes[id]
    }

    /// The root node id.
    #[must_use]
    pub fn root(&self) -> usize {
        self.root
    }

    /// Whether `ancestor` is `id` itself or one of its ancestors.
    #[must_use]
    pub fn in_subtree(&self, ancestor: usize, mut id: usize) -> bool {
        loop {
            if id == ancestor {
                return true;
            }
            match self.nodes[id].parent {
                Some(p) => id = p,
                None => return false,
            }
        }
    }

    /// Where an experiment belongs: the node id of the first matching
    /// assignment (`(unassigned)` when nothing matches) and its effective
    /// criticality weight (`defaults.weight × criticality level`).
    #[must_use]
    pub fn assign(&self, experiment_name: &str) -> (usize, f64) {
        for a in &self.assignments {
            if a.targets.iter().any(|t| glob_match(t, experiment_name)) {
                let level = a
                    .criticality
                    .get(experiment_name)
                    .map_or(1, |l| criticality_weight(l));
                return (a.node, self.default_weight * f64::from(level));
            }
        }
        (self.unassigned, self.default_weight)
    }

    /// Aggregate one node from the full leaf list: score, band, coverage,
    /// weakest member and one level of child rollups (weakest first).
    /// The score is the criticality-weighted mean over ALL scored leaves in
    /// the subtree — recomputed from leaves, never from child means.
    #[must_use]
    pub fn compute_node(&self, path: &str, leaves: &[ScoredLeaf]) -> Option<OrgNodeScore> {
        let id = self.resolve(path)?;
        Some(self.aggregate(id, leaves, true))
    }

    fn aggregate(&self, id: usize, leaves: &[ScoredLeaf], with_children: bool) -> OrgNodeScore {
        let mut sum = 0.0;
        let mut weights = 0.0;
        let mut total_weight = 0.0;
        let mut scored = 0usize;
        let mut expected = 0usize;
        let mut weakest: Option<(u32, &str)> = None;
        for leaf in leaves {
            let (node, weight) = self.assign(&leaf.name);
            if !self.in_subtree(id, node) {
                continue;
            }
            expected += 1;
            total_weight += weight;
            if let Some(score) = leaf.score {
                scored += 1;
                sum += weight * f64::from(score);
                weights += weight;
                if weakest.is_none_or(|(s, _)| score < s) {
                    weakest = Some((score, leaf.name.as_str()));
                }
            }
        }
        let score = if weights > 0.0 { sum / weights } else { 0.0 };
        let children = if with_children {
            let mut c: Vec<OrgNodeScore> = self.nodes[id]
                .children
                .iter()
                .map(|&child| self.aggregate(child, leaves, false))
                .collect();
            c.sort_by(|a, b| {
                a.score
                    .partial_cmp(&b.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            c
        } else {
            vec![]
        };
        OrgNodeScore {
            path: self.path_of(id),
            name: if id == self.root {
                "(company)".to_string()
            } else {
                self.nodes[id].name.clone()
            },
            kind: self.nodes[id].kind.clone(),
            score,
            band: crate::scoring::band(score).to_string(),
            coverage: if expected == 0 {
                0.0
            } else {
                scored as f64 / expected as f64
            },
            scored,
            expected,
            weakest: weakest.map(|(_, n)| n.to_string()),
            weight: total_weight,
            children,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const YAML: &str = "
nodes:
  - {name: platform, kind: domain}
  - {name: edge, kind: unit, parent: platform}
  - {name: compute, kind: unit, parent: platform}
  - {name: edge-team, parent: edge}
  - {name: db-team, parent: compute}
assignments:
  - team: edge-team
    targets: [\"edge-*\"]
    criticality: {edge-cdn-outage: critical, edge-cache-miss: high}
  - team: db-team
    targets: [\"db-*\", \"pg-*\"]
defaults: {weight: 1.0, stale_days: 30}
";

    fn tree() -> OrgTree {
        OrgTree::from_yaml(YAML).unwrap()
    }

    #[test]
    fn glob_star_variants() {
        assert!(glob_match("edge-*", "edge-cdn-outage"));
        assert!(glob_match("*-outage", "edge-cdn-outage"));
        assert!(glob_match("*cdn*", "edge-cdn-outage"));
        assert!(glob_match("edge-cdn-outage", "edge-cdn-outage"));
        assert!(glob_match("*", "anything"));
        assert!(!glob_match("edge-*", "db-failover"));
        assert!(!glob_match("*-outage", "edge-cdn"));
        assert!(!glob_match("edge-*-outage", "edge-outage-x"));
        assert!(glob_match("edge-*-outage", "edge-cdn-outage"));
        assert!(!glob_match("db-*", "db")); // trailing star still needs the prefix
        assert!(glob_match("db*", "db"));
    }

    #[test]
    fn validation_rejects_bad_files() {
        // Unknown parent.
        assert!(OrgTree::from_yaml("nodes: [{name: a, parent: nope}]").is_err());
        // Duplicate name.
        assert!(OrgTree::from_yaml("nodes: [{name: a}, {name: a}]").is_err());
        // Reserved name.
        assert!(OrgTree::from_yaml("nodes: [{name: \"(unassigned)\"}]").is_err());
        // Slash in name.
        assert!(OrgTree::from_yaml("nodes: [{name: \"a/b\"}]").is_err());
        // Cycle (a parent b, b parent a) — needs both names resolvable.
        let cyc = "nodes: [{name: a, parent: b}, {name: b, parent: a}]";
        assert!(OrgTree::from_yaml(cyc).is_err());
        // Assignment to unknown team.
        let bad_team = "nodes: [{name: t}]\nassignments: [{team: ghost, targets: [\"x-*\"]}]";
        assert!(OrgTree::from_yaml(bad_team).is_err());
    }

    #[test]
    fn paths_resolve_and_round_trip() {
        let t = tree();
        let root = t.resolve("").unwrap();
        assert_eq!(t.path_of(root), "");
        let edge = t.resolve("platform/edge").unwrap();
        assert_eq!(t.path_of(edge), "platform/edge");
        assert!(t.resolve("platform/nope").is_none());
        assert!(t.resolve("edge").is_none()); // must start at a root child
        assert_eq!(t.resolve("/platform/edge/"), Some(edge));
        let un = t.resolve("(unassigned)").unwrap();
        assert_eq!(t.path_of(un), "(unassigned)");
    }

    #[test]
    fn assignment_globs_and_criticality() {
        let t = tree();
        let edge_team = t.resolve("platform/edge/edge-team").unwrap();
        let (node, w) = t.assign("edge-cdn-outage");
        assert_eq!((node, w), (edge_team, 3.0)); // critical
        let (_, w) = t.assign("edge-cache-miss");
        assert_eq!(w, 2.0); // high
        let (_, w) = t.assign("edge-other");
        assert_eq!(w, 1.0); // default
        let db_team = t.resolve("platform/compute/db-team").unwrap();
        assert_eq!(t.assign("pg-failover").0, db_team);
        // Unmapped -> (unassigned) at root with default weight.
        let un = t.resolve("(unassigned)").unwrap();
        assert_eq!(t.assign("mystery-exp"), (un, 1.0));
    }

    #[test]
    fn node_score_is_weighted_mean_over_leaves_not_child_means() {
        let t = tree();
        // edge-team: one critical leaf at 100. db-team: three leaves at 50.
        // Average of child means would give (100+50)/2 = 75 for "platform";
        // the correct leaf-recomputed mean is (3*100 + 3*50)/(3+3) = 75 for
        // weights, but (100+50+50+50)/4 = 62.5 without the criticality
        // weight — test both levels explicitly.
        let leaves = vec![
            ScoredLeaf {
                name: "edge-cdn-outage".into(),
                score: Some(100),
            },
            ScoredLeaf {
                name: "db-a".into(),
                score: Some(50),
            },
            ScoredLeaf {
                name: "db-b".into(),
                score: Some(50),
            },
            ScoredLeaf {
                name: "pg-c".into(),
                score: Some(50),
            },
        ];
        let edge = t.compute_node("platform/edge", &leaves).unwrap();
        assert_eq!(edge.score, 100.0);
        assert_eq!(edge.coverage, 1.0);
        assert_eq!(edge.expected, 1);
        let platform = t.compute_node("platform", &leaves).unwrap();
        // Σ w·s = 3*100 + 1*50*3 = 450; Σ w = 3 + 3 = 6 → 75.0.
        assert_eq!(platform.score, 75.0);
        assert_eq!(platform.expected, 4);
        assert_eq!(platform.scored, 4);
        assert_eq!(platform.weakest.as_deref(), Some("db-a"));
        // Without the critical weight, the same leaves give 62.5 — proving
        // the weight is applied per leaf.
        let leaves2 = vec![
            ScoredLeaf {
                name: "edge-other".into(),
                score: Some(100),
            },
            ScoredLeaf {
                name: "db-a".into(),
                score: Some(50),
            },
            ScoredLeaf {
                name: "db-b".into(),
                score: Some(50),
            },
            ScoredLeaf {
                name: "pg-c".into(),
                score: Some(50),
            },
        ];
        let platform2 = t.compute_node("platform", &leaves2).unwrap();
        assert_eq!(platform2.score, 62.5);
        assert_eq!(platform2.band, "fair");
    }

    #[test]
    fn unassigned_bucket_and_pending_coverage() {
        let t = tree();
        let leaves = vec![
            ScoredLeaf {
                name: "edge-other".into(),
                score: Some(80),
            },
            ScoredLeaf {
                name: "mystery-exp".into(),
                score: Some(100),
            },
            // Pending manual draft: expected but no score weight.
            ScoredLeaf {
                name: "edge-drill".into(),
                score: None,
            },
        ];
        let root = t.compute_node("", &leaves).unwrap();
        assert_eq!(root.expected, 3);
        assert_eq!(root.scored, 2);
        assert!((root.coverage - 2.0 / 3.0).abs() < 1e-9);
        // Children: unassigned (100) and edge-team subtree ((80+? )…).
        let un = root
            .children
            .iter()
            .find(|c| c.name == UNASSIGNED)
            .expect("unassigned child");
        assert_eq!(un.score, 100.0);
        assert_eq!(un.expected, 1);
        // Edge subtree: one scored (80) + one pending → score 80, coverage 1/2.
        let edge = t.compute_node("platform/edge", &leaves).unwrap();
        assert_eq!(edge.score, 80.0);
        assert_eq!(edge.scored, 1);
        assert_eq!(edge.expected, 2);
        // Children sorted weakest first: compute has no leaves (score 0)
        // and sorts before edge (80).
        let platform = t.compute_node("platform", &leaves).unwrap();
        assert_eq!(platform.children[0].name, "compute");
    }

    #[test]
    fn empty_subtree_scores_zero_with_zero_coverage() {
        let t = tree();
        let node = t.compute_node("platform/compute", &[]).unwrap();
        assert_eq!(node.score, 0.0);
        assert_eq!(node.coverage, 0.0);
        assert_eq!(node.band, "poor");
        assert_eq!(node.weight, 0.0);
        // A completely empty tree still rolls up the root.
        let empty = OrgTree::empty();
        let root = empty.compute_node("", &[]).unwrap();
        assert_eq!(root.expected, 0);
        assert_eq!(root.children.len(), 1); // just (unassigned)
    }
}
