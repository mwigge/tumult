//! Dispatch bodies for `ChaosGraph` tools: `chaosgraph_query`,
//! `chaosgraph_neighbors`, and `chaosgraph_coverage_gaps`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::handler::schema::{
    ChaosGraphCoverageGapsTool, ChaosGraphNeighborsTool, ChaosGraphQueryTool,
};
use crate::tools;

use super::{parse_args, Dispatched, ToolOutput};

pub(super) fn chaosgraph_query(params: &CallToolRequestParams) -> Dispatched {
    let args: ChaosGraphQueryTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::chaosgraph_query(&args.store_path, &args.kind, args.filter.as_deref())
    })
    .map(ToolOutput::from))
}

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

pub(super) fn chaosgraph_cypher(params: &CallToolRequestParams) -> Dispatched {
    let args: crate::handler::schema::ChaosGraphCypherTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::chaosgraph_cypher(&args.store_path, &args.query, args.row_cap)
    })
    .map(ToolOutput::from))
}
