//! The Star Map: one graph of organisation, entities, systems, services,
//! workloads and the evidence against them, assembled per request from
//! rows the caller already has. Pure: no I/O, nothing persisted.

mod build;
mod cluster;
mod graph;
mod input;
mod model;
mod overlay;
mod prune;
mod spine;
#[cfg(test)]
mod tests;

pub use input::{
    ArticleRow, CallRow, CoverFrom, CoverRow, DefinitionRow, DependsRow, ExperimentRow, GapRow,
    GlobRow, OrgUnitRow, PostureRow, RiskRow, RunRow, ServiceRow, StarInputs, StarQuery,
    SuppressionRow, SystemRow, WorkloadRow, DEFAULT_BUDGET, DEFAULT_DEPTH, MAX_BUDGET, MAX_DEPTH,
};
pub use model::{
    ClusterWorst, Counts, EdgeSource, Evidence, Gap, Posture, RiskSummary, Sources, StarCluster,
    StarEdge, StarKind, StarMap, StarNode, StarRel,
};

/// Value of a `sources` entry whose source does not exist yet.
pub const UNAVAILABLE: &str = "unavailable";

/// Assemble the Star Map for one request.
///
/// Builds nodes and typed edges from the rows, resolves owners (telemetry
/// `ict.entity.owner`, then `org.yaml` entity globs, then the
/// experiment-name assignment, else `(unassigned)`), picks one spine
/// parent per node, attaches the overlay blocks, enforces the caller's
/// environments, applies `focus`/`depth` and `kinds`, clusters to the
/// budget and sorts by id. The same inputs in any row order give
/// byte-identical output.
#[must_use]
pub fn assemble(inputs: &StarInputs, query: &StarQuery) -> StarMap {
    let scope = query.scope.as_deref();
    let mut builder = build::Builder::new(inputs, scope, query.include_runs);
    builder.build();
    builder.assign_spine();
    builder.attach_overlays();
    let mut graph = builder.graph;
    prune::apply_scope(&mut graph, scope);

    let before = graph.nodes.len();
    let depth = query
        .focus
        .as_ref()
        .map(|_| query.depth.unwrap_or(DEFAULT_DEPTH).clamp(1, MAX_DEPTH));
    if let (Some(focus), Some(depth)) = (&query.focus, depth) {
        prune::apply_focus(&mut graph, focus, depth);
    }
    if let Some(kinds) = &query.kinds {
        prune::apply_kinds(&mut graph, kinds);
    }
    let pruned = before - graph.nodes.len();
    let (clusters, clustered) = cluster::apply_budget(&mut graph, query.budget);
    count_children(&mut graph);

    let nodes: Vec<StarNode> = graph.nodes.into_values().collect();
    let edges: Vec<StarEdge> = graph.edges.into_values().collect();
    StarMap {
        as_of_ns: inputs.as_of_ns,
        range: query.range.clone(),
        env: query.env.clone(),
        focus: query.focus.clone(),
        depth,
        disclaimer: tumult_core::compliance::EVIDENCE_DISCLAIMER,
        sources: Sources {
            slo: UNAVAILABLE,
            dora: UNAVAILABLE,
            maturity: UNAVAILABLE,
            conformance: "ok",
            graph: "ok",
        },
        maturity: None,
        counts: counts(&nodes, edges.len(), pruned, clustered),
        nodes,
        edges,
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

fn counts(nodes: &[StarNode], edges: usize, pruned: usize, clustered: usize) -> Counts {
    // An entity's risks repeat those of its systems, so it is not summed.
    let own = || nodes.iter().filter(|n| n.kind != StarKind::Entity);
    Counts {
        nodes: nodes.len(),
        edges,
        pruned,
        clustered,
        risks_open: own().map(|n| n.risks.open).sum(),
        risks_suppressed: own().map(|n| n.risks.suppressed).sum(),
        gaps: nodes.iter().filter(|n| n.gap.is_some()).count(),
    }
}
