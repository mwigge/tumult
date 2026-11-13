//! Counts over what hangs below an entity or an org unit.

use std::collections::{BTreeMap, BTreeSet};

use super::graph::Graph;
use super::model::{Gap, Rollup, StarKind, StarNode};

fn add_system(into: &mut Rollup, system: &StarNode) {
    into.systems += 1;
    into.untested_systems += usize::from(system.gap == Some(Gap::Missing));
    into.stale_systems += usize::from(system.gap == Some(Gap::Stale));
    into.failed += system.evidence.failed;
}

/// What one node adds to every org unit above it. An entity adds
/// nothing of its own: its numbers repeat those of its systems.
fn contribution(node: &StarNode) -> Rollup {
    let mut own = Rollup::default();
    if node.kind == StarKind::System {
        add_system(&mut own, node);
    }
    own.risks_open = node.risks.open;
    own.risks_suppressed = node.risks.suppressed;
    own.gaps = usize::from(node.gap.is_some());
    own
}

fn add(into: &mut Rollup, from: &Rollup) {
    into.systems += from.systems;
    into.untested_systems += from.untested_systems;
    into.stale_systems += from.stale_systems;
    into.failed += from.failed;
    into.risks_open += from.risks_open;
    into.risks_suppressed += from.risks_suppressed;
    into.gaps += from.gaps;
}

/// The org units above a node on the spine, nearest first.
fn org_ancestors(graph: &Graph, id: &str) -> Vec<String> {
    let mut above = Vec::new();
    let mut parent = graph.nodes.get(id).and_then(|n| n.parent.as_deref());
    for _ in 0..graph.nodes.len() {
        let Some(node) = parent.and_then(|p| graph.nodes.get(p)) else {
            break;
        };
        if node.kind == StarKind::Org {
            above.push(node.id.clone());
        }
        parent = node.parent.as_deref();
    }
    above
}

fn entity_rollup(graph: &Graph, systems: Option<&BTreeSet<String>>) -> Rollup {
    let mut rollup = Rollup::default();
    for system in systems.into_iter().flatten() {
        let Some(system) = graph.nodes.get(system) else {
            continue;
        };
        add(&mut rollup, &contribution(system));
    }
    rollup
}

/// Give every entity the counts over the systems it implements and every
/// org unit the counts over its spine subtree.
pub(super) fn apply(graph: &mut Graph, ent_systems: &BTreeMap<String, BTreeSet<String>>) {
    let mut rollups: BTreeMap<String, Rollup> = BTreeMap::new();
    for node in graph.nodes.values() {
        match node.kind {
            StarKind::Org => {
                rollups.entry(node.id.clone()).or_default();
            }
            StarKind::Entity => {
                let own = entity_rollup(graph, ent_systems.get(&node.id));
                rollups.insert(node.id.clone(), own);
            }
            _ => {
                let own = contribution(node);
                for unit in org_ancestors(graph, &node.id) {
                    add(rollups.entry(unit).or_default(), &own);
                }
            }
        }
    }
    for (id, rollup) in rollups {
        if let Some(node) = graph.nodes.get_mut(&id) {
            node.rollup = Some(rollup);
        }
    }
}
