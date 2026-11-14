//! Pruning: environment scope, focus neighbourhood and kind filter.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::build::{in_scope, UNASSIGNED};
use super::graph::Graph;
use super::model::StarKind;

type Parents = BTreeMap<String, Option<String>>;

fn parents_of(graph: &Graph) -> Parents {
    graph
        .nodes
        .values()
        .map(|n| (n.id.clone(), n.parent.clone()))
        .collect()
}

/// Point every node whose parent was removed at its nearest surviving
/// ancestor, or at nothing when the whole chain is gone.
fn reparent(graph: &mut Graph, original: &Parents) {
    let alive: BTreeSet<String> = graph.nodes.keys().cloned().collect();
    for node in graph.nodes.values_mut() {
        let mut parent = node.parent.clone();
        let mut hops = 0;
        while let Some(id) = parent.clone() {
            if alive.contains(&id) || hops > original.len() {
                break;
            }
            parent = original.get(&id).cloned().flatten();
            hops += 1;
        }
        node.parent = parent.filter(|id| alive.contains(id));
    }
}

fn is_placed(kind: StarKind) -> bool {
    matches!(
        kind,
        StarKind::System
            | StarKind::Component
            | StarKind::Service
            | StarKind::Workload
            | StarKind::Ext
    )
}

fn is_satellite(kind: StarKind) -> bool {
    matches!(
        kind,
        StarKind::Def | StarKind::Run | StarKind::Slo | StarKind::Gap
    )
}

/// Enforce the caller's environments. Systems, components, services and
/// workloads must carry an allowed environment; entities need a visible
/// implementer; definitions, runs and articles need a visible neighbour.
/// With no scope everything stays, including nodes with no environment.
pub(super) fn apply_scope(graph: &mut Graph, scope: Option<&[String]>) {
    if scope.is_none() {
        return;
    }
    let original = parents_of(graph);
    graph
        .nodes
        .retain(|_, n| !is_placed(n.kind) || in_scope(scope, n.env.as_deref()));
    graph.drop_dangling();
    let anchored = anchored_ids(graph);
    let reached = reach_satellites(graph, &anchored);
    let cited = cited_articles(graph, &reached);
    graph.nodes.retain(|id, n| {
        n.kind == StarKind::Org
            || anchored.contains(id)
            || reached.contains(id)
            || cited.contains(id)
    });
    graph.drop_dangling();
    reparent(graph, &original);
    drop_empty_unassigned(graph);
}

/// Articles a surviving definition or run covers.
fn cited_articles(graph: &Graph, reached: &BTreeSet<String>) -> BTreeSet<String> {
    graph
        .edges
        .keys()
        .filter(|(from, _, _)| reached.contains(from))
        .map(|(_, _, to)| to)
        .filter(|to| {
            graph
                .nodes
                .get(*to)
                .is_some_and(|n| n.kind == StarKind::Article)
        })
        .cloned()
        .collect()
}

/// Placed nodes, plus the entities at least one of them implements.
fn anchored_ids(graph: &Graph) -> BTreeSet<String> {
    let kind = |id: &str| graph.nodes.get(id).map(|n| n.kind);
    let mut anchored: BTreeSet<String> = graph
        .nodes
        .values()
        .filter(|n| is_placed(n.kind))
        .map(|n| n.id.clone())
        .collect();
    for (from, _, to) in graph.edges.keys() {
        let pair = (kind(from), kind(to));
        if let (Some(StarKind::Entity), Some(other)) = pair {
            if is_placed(other) {
                anchored.insert(from.clone());
            }
        }
        if let (Some(other), Some(StarKind::Entity)) = pair {
            if is_placed(other) {
                anchored.insert(to.clone());
            }
        }
    }
    anchored
}

/// Satellites connected, directly or through other satellites, to an
/// anchored node.
fn reach_satellites(graph: &Graph, anchored: &BTreeSet<String>) -> BTreeSet<String> {
    let adjacency = graph.adjacency();
    let mut reached: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<&str> = anchored.iter().map(String::as_str).collect();
    while let Some(id) = queue.pop_front() {
        for next in adjacency.get(id).into_iter().flatten() {
            let satellite = graph.nodes.get(*next).is_some_and(|n| is_satellite(n.kind));
            if satellite && reached.insert((*next).to_string()) {
                queue.push_back(next);
            }
        }
    }
    reached
}

fn drop_empty_unassigned(graph: &mut Graph) {
    let used = graph
        .nodes
        .values()
        .any(|n| n.parent.as_deref() == Some(UNASSIGNED));
    if !used {
        graph.nodes.remove(UNASSIGNED);
        graph.drop_dangling();
    }
}

/// Keep the nodes within `depth` hops of `focus` over every edge and
/// spine link, plus the spine above them. An unknown focus keeps nothing
/// and returns `false`.
pub(super) fn apply_focus(graph: &mut Graph, focus: &str, depth: u8) -> bool {
    if !graph.has(focus) {
        graph.nodes.clear();
        graph.edges.clear();
        return false;
    }
    let mut keep = neighbourhood(graph, focus, depth);
    for id in keep.clone() {
        let mut parent = graph.nodes.get(&id).and_then(|n| n.parent.clone());
        while let Some(up) = parent {
            if !keep.insert(up.clone()) {
                break;
            }
            parent = graph.nodes.get(&up).and_then(|n| n.parent.clone());
        }
    }
    graph.retain(&keep);
    true
}

fn neighbourhood(graph: &Graph, focus: &str, depth: u8) -> BTreeSet<String> {
    let adjacency = graph.adjacency();
    let mut keep: BTreeSet<String> = std::iter::once(focus.to_string()).collect();
    let mut queue: VecDeque<(&str, u8)> = std::iter::once((focus, 0)).collect();
    while let Some((id, hops)) = queue.pop_front() {
        if hops == depth {
            continue;
        }
        for next in adjacency.get(id).into_iter().flatten() {
            if keep.insert((*next).to_string()) {
                queue.push_back((next, hops + 1));
            }
        }
    }
    keep
}

/// Keep only the requested kinds; survivors hang from their nearest
/// surviving ancestor.
pub(super) fn apply_kinds(graph: &mut Graph, kinds: &[StarKind]) {
    let original = parents_of(graph);
    let keep: BTreeSet<String> = graph
        .nodes
        .values()
        .filter(|n| kinds.contains(&n.kind))
        .map(|n| n.id.clone())
        .collect();
    graph.retain(&keep);
    reparent(graph, &original);
}
