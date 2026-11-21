//! The Star Map: one graph of organisation, entities, systems, services,
//! workloads and the evidence against them, assembled per request from
//! rows the caller already has. Pure: no I/O, nothing persisted.

mod build;
mod cluster;
mod graph;
mod ids;
mod input;
mod model;
mod overlay;
mod prune;
mod rollup;
mod spine;
#[cfg(test)]
mod tests;

pub use input::{
    ArticleRow, CallRow, CoverFrom, CoverRow, DefinitionRow, DependsRow, ExperimentRow, GapRow,
    OrgUnitRow, PostureRow, RiskRow, RiskSubject, RunRow, ServiceRow, StarInputs, StarQuery,
    SuppressionRow, SystemRow, WorkloadRow, DEFAULT_BUDGET, DEFAULT_DEPTH, MAX_BUDGET, MAX_DEPTH,
};
pub use model::{
    ClusterWorst, Counts, EdgeSource, Evidence, Flag, Gap, Posture, RiskSummary, Rollup,
    StarCluster, StarEdge, StarKind, StarMap, StarNode, StarRel,
};

/// The node id a `focus` names. Entity ids carry the lower-case form
/// of the id as reported (entities compare without regard to case), so
/// an entity focus is folded the same way; every other id is taken as
/// given.
fn focus_id(asked: &str) -> String {
    match asked.strip_prefix("ent:") {
        Some(entity) => format!("ent:{}", entity.to_lowercase()),
        None => asked.to_string(),
    }
}

/// Assemble the Star Map for one request.
///
/// Builds nodes and typed edges from the rows, picks owners (telemetry
/// `ict.entity.owner`, then the `org.yaml` assignment the caller resolved
/// for the entity, then the one resolved for its experiments, else
/// `(unassigned)`), picks one spine parent per node, attaches the overlay
/// blocks, enforces the caller's environments, rolls counts up to
/// entities and org units, applies `focus`/`depth` and `kinds`, clusters
/// to the budget and sorts by id. The same inputs in any row order give
/// byte-identical output.
#[must_use]
pub fn assemble(inputs: &StarInputs, query: &StarQuery) -> StarMap {
    let scope = query.scope.as_deref();
    let mut builder = build::Builder::new(inputs, scope, query.include_runs);
    builder.build();
    builder.assign_spine();
    builder.attach_overlays();
    let build::Builder {
        mut graph,
        facts,
        risks_unplaced,
        ..
    } = builder;
    prune::apply_scope(&mut graph, scope);
    rollup::apply(&mut graph, &facts.ent_systems);

    let before = graph.nodes.len();
    let focus = query.focus.as_deref().map(focus_id);
    let depth = focus
        .as_ref()
        .map(|_| query.depth.unwrap_or(DEFAULT_DEPTH).clamp(1, MAX_DEPTH));
    let focus_found = match (&focus, depth) {
        (Some(focus), Some(depth)) => Some(prune::apply_focus(&mut graph, focus, depth)),
        _ => None,
    };
    if let Some(kinds) = &query.kinds {
        prune::apply_kinds(&mut graph, kinds);
    }
    let pruned = before - graph.nodes.len();
    // Counted before clustering, so the totals do not move with the budget.
    let mut counts = overlay_counts(&graph);
    counts.pruned = pruned;
    counts.risks_unplaced = risks_unplaced;
    let (clusters, clustered) = cluster::apply_budget(&mut graph, query.budget);
    count_children(&mut graph);
    counts.clustered = clustered;
    counts.nodes = graph.nodes.len();
    counts.edges = graph.edges.len();

    StarMap {
        as_of_ns: inputs.as_of_ns,
        range: query.range.clone(),
        env: query.env.clone(),
        focus,
        focus_found,
        depth,
        disclaimer: tumult_core::compliance::EVIDENCE_DISCLAIMER,
        truncated: inputs.truncated,
        maturity: None,
        counts,
        nodes: graph.nodes.into_values().collect(),
        edges: graph.edges.into_values().collect(),
        clusters,
    }
}

fn count_children(graph: &mut graph::Graph) {
    let parents: Vec<String> = graph
        .nodes
        .values()
        .filter_map(|n| n.parent.clone())
        .collect();
    for parent in parents {
        if let Some(node) = graph.nodes.get_mut(&parent) {
            node.children += 1;
        }
    }
}

/// Risk and gap totals over the nodes the request kept.
fn overlay_counts(graph: &graph::Graph) -> Counts {
    // An entity's risks repeat those of its systems, so it is not summed.
    let own = || graph.nodes.values().filter(|n| n.kind != StarKind::Entity);
    Counts {
        risks_open: own().map(|n| n.risks.open).sum(),
        risks_suppressed: own().map(|n| n.risks.suppressed).sum(),
        gaps: graph.nodes.values().filter(|n| n.gap.is_some()).count(),
        ..Counts::default()
    }
}
