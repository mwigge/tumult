//! Budget clustering: when the pruned graph is still too large, the
//! deepest rings collapse into one cluster node per org unit.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};

use super::build::ROOT;
use super::graph::Graph;
use super::model::{ClusterWorst, Gap, RiskSummary, StarCluster, StarEdge, StarKind};
use super::overlay::merge_risks;

/// Deepest ring that can collapse (satellites) and the shallowest
/// (entities). Org units and articles never collapse.
const DEEPEST_RING: u8 = 6;
const SHALLOWEST_RING: u8 = 2;

/// Fold traffic numbers into an edge: counts add up, the mean duration
/// is weighted by call count.
#[allow(clippy::cast_precision_loss)] // call counts are far below 2^52
pub(super) fn add_traffic(
    edge: &mut StarEdge,
    weight: Option<u64>,
    errors: Option<u64>,
    avg_duration_ns: Option<f64>,
) {
    let held = edge.weight.unwrap_or(0);
    let added = weight.unwrap_or(0);
    if let Some(avg) = avg_duration_ns {
        let total = held + added;
        edge.avg_duration_ns = Some(match edge.avg_duration_ns {
            Some(current) if total > 0 => {
                (current * held as f64 + avg * added as f64) / total as f64
            }
            _ => avg,
        });
    }
    if weight.is_some() {
        edge.weight = Some(held + added);
    }
    if let Some(errors) = errors {
        edge.errors = Some(edge.errors.unwrap_or(0) + errors);
    }
}

#[derive(Debug, Default)]
struct Collapsed {
    members: usize,
    gap: Option<Gap>,
    risks: RiskSummary,
}

fn within(graph: &Graph, budget: usize) -> bool {
    graph.nodes.len() <= budget && graph.edges.len() <= budget.saturating_mul(8) / 3
}

/// The org unit above a node on the spine.
fn spine_home(graph: &Graph, id: &str) -> Option<String> {
    let mut current = graph.nodes.get(id)?;
    for _ in 0..graph.nodes.len() {
        if current.kind == StarKind::Org {
            return Some(current.id.clone());
        }
        current = graph.nodes.get(current.parent.as_deref()?)?;
    }
    None
}

/// The org unit each node collapses into: its own spine, else the spine
/// of its first neighbour, else the root.
fn homes(graph: &Graph) -> BTreeMap<String, String> {
    let adjacency = graph.adjacency();
    graph
        .nodes
        .keys()
        .map(|id| {
            let home = spine_home(graph, id)
                .or_else(|| {
                    let near = adjacency.get(id.as_str())?;
                    near.iter().find_map(|n| spine_home(graph, n))
                })
                .unwrap_or(ROOT.to_string());
            (id.clone(), home)
        })
        .collect()
}

/// Collapse rings, deepest first, until the graph fits the budget.
/// Within a ring the largest org unit goes first, ties by path.
pub(super) fn apply_budget(graph: &mut Graph, budget: usize) -> (Vec<StarCluster>, usize) {
    if within(graph, budget) {
        return (Vec::new(), 0);
    }
    let homes = homes(graph);
    let mut collapsed: BTreeMap<String, Collapsed> = BTreeMap::new();
    for ring in (SHALLOWEST_RING..=DEEPEST_RING).rev() {
        let mut groups: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
        for node in graph.nodes.values().filter(|n| n.kind.ring() == ring) {
            let home = homes.get(&node.id).map_or(ROOT, String::as_str);
            groups.entry(home).or_default().insert(node.id.clone());
        }
        let mut order: Vec<(&str, BTreeSet<String>)> = groups.into_iter().collect();
        order.sort_by_key(|(home, ids)| (Reverse(ids.len()), *home));
        for (home, ids) in order {
            if within(graph, budget) {
                break;
            }
            collapse(graph, home, &ids, &mut collapsed);
        }
    }
    summarise(graph, &collapsed)
}

fn collapse(
    graph: &mut Graph,
    home: &str,
    ids: &BTreeSet<String>,
    collapsed: &mut BTreeMap<String, Collapsed>,
) {
    let cluster_id = format!("cluster:{home}");
    let state = collapsed.entry(cluster_id.clone()).or_default();
    for id in ids {
        let Some(node) = graph.nodes.remove(id) else {
            continue;
        };
        state.members += 1;
        state.gap = state.gap.max(node.gap);
        // An entity's risks are a roll-up of its systems' risks.
        if node.kind != StarKind::Entity {
            merge_risks(&mut state.risks, &node.risks);
        }
    }
    let unit = graph.nodes.get(home).map(|n| n.label.clone());
    let name = unit.clone().unwrap_or_else(|| home.to_string());
    let node = graph.ensure(&cluster_id, StarKind::Cluster, "", false);
    node.label = format!("{name} ({})", state.members);
    node.parent = unit.map(|_| home.to_string());
    node.members = Some(state.members);
    node.gap = state.gap;
    node.risks = state.risks.clone();
    redirect(graph, ids, &cluster_id);
}

/// Re-point edges and parents at the cluster; edges that become
/// parallel merge, edges that become loops go.
fn redirect(graph: &mut Graph, moved: &BTreeSet<String>, cluster_id: &str) {
    let target = |id: String| {
        if moved.contains(&id) {
            return cluster_id.to_string();
        }
        id
    };
    for (_, edge) in std::mem::take(&mut graph.edges) {
        let from = target(edge.from);
        let to = target(edge.to);
        if from == to {
            continue;
        }
        let merged = graph.link(&from, edge.rel, &to, edge.source);
        add_traffic(merged, edge.weight, edge.errors, edge.avg_duration_ns);
    }
    for node in graph.nodes.values_mut() {
        if node.parent.as_ref().is_some_and(|p| moved.contains(p)) {
            node.parent = Some(cluster_id.to_string());
        }
    }
}

fn summarise(graph: &Graph, collapsed: &BTreeMap<String, Collapsed>) -> (Vec<StarCluster>, usize) {
    let clusters: Vec<StarCluster> = collapsed
        .iter()
        .filter_map(|(id, state)| {
            let node = graph.nodes.get(id)?;
            Some(StarCluster {
                id: id.clone(),
                label: node.label.clone(),
                members: state.members,
                worst: ClusterWorst {
                    gap: state.gap,
                    risk: state.risks.worst.clone(),
                },
            })
        })
        .collect();
    let total = clusters.iter().map(|c| c.members).sum();
    (clusters, total)
}
