//! Dispatch bodies for `ChaosGraph` tools: `chaosgraph_query`,
//! `chaosgraph_neighbors`, and `chaosgraph_coverage_gaps`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::handler::schema::{
    ChaosGraphCoverageGapsTool, ChaosGraphNeighborsTool, ChaosGraphQueryTool,
};
use crate::handler::Role;
use crate::tools;

use super::{parse_args, store_path_for, Dispatched, ToolOutput};

/// Dispatch `tumult_chaosgraph_query`: list graph node ids and one-line
/// summaries for a kind from the persistent analytics store.
pub(super) fn chaosgraph_query(params: &CallToolRequestParams) -> Dispatched {
    let args: ChaosGraphQueryTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::chaosgraph_query(&args.store_path, &args.kind, args.filter.as_deref())
    })
    .map(ToolOutput::from))
}

/// Dispatch `tumult_chaosgraph_neighbors`: return the ego sub-graph of a node
/// within `depth` hops, optionally filtered to a single relation.
pub(super) fn chaosgraph_neighbors(params: &CallToolRequestParams) -> Dispatched {
    let args: ChaosGraphNeighborsTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::chaosgraph_neighbors(
            &args.store_path,
            &args.node_id,
            args.rel.as_deref(),
            args.depth,
        )
    })
    .map(ToolOutput::from))
}

/// Dispatch `tumult_chaosgraph_coverage_gaps`: list catalog actions never
/// exercised by a tested run (plus unevidenced framework articles when a
/// framework is given). Always derived read-only here — a read tool never
/// takes the store's write lock.
pub(super) fn chaosgraph_coverage_gaps(params: &CallToolRequestParams) -> Dispatched {
    let args: ChaosGraphCoverageGapsTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        // The server must never take the store's write lock from a
        // read tool, so coverage gaps are always derived read-only
        // here (refresh = false).
        tools::chaosgraph_coverage_gaps(
            &args.store_path,
            args.framework.as_deref(),
            args.domain.as_deref(),
            false,
        )
    })
    .map(ToolOutput::from))
}

/// Dispatch `tumult_chaosgraph_cypher`: run a read-only openCypher query over
/// an in-memory snapshot of the store's graph. Viewer-role callers are pinned
/// to the default store path (see `store_path_for`).
pub(super) fn chaosgraph_cypher(params: &CallToolRequestParams, role: Option<Role>) -> Dispatched {
    let args: crate::handler::schema::ChaosGraphCypherTool = parse_args(params)?;
    let store_path = store_path_for(role, &args.store_path);
    Ok(tokio::task::block_in_place(|| {
        tools::chaosgraph_cypher(&store_path, &args.query, args.row_cap)
    })
    .map(ToolOutput::from))
}
