//! Dispatch bodies for topology tools: `topology_import`, `topology_map`,
//! `compliance_lineage`, and `recommend_injection`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::handler::schema::{
    ComplianceLineageTool, RecommendInjectionTool, TopologyImportTool, TopologyMapTool,
};
use crate::tools;

use super::{parse_args, Dispatched, ToolOutput};

/// Dispatch `tumult_topology_import`: parse a declared topology TOML (inline
/// or from a file — exactly one) and replace the store's declared-topology
/// sub-graph. The one topology tool that opens the store read-write.
pub(super) fn topology_import(params: &CallToolRequestParams) -> Dispatched {
    let args: TopologyImportTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::topology_import(
            &args.store_path,
            args.toml_content.as_deref(),
            args.path.as_deref(),
        )
    })
    .map(ToolOutput::from))
}

/// Dispatch `tumult_topology_map`: render the compliance-aware service map as
/// text, Mermaid, or JSON. Reads the store read-only.
pub(super) fn topology_map(params: &CallToolRequestParams) -> Dispatched {
    let args: TopologyMapTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::topology_map(
            &args.store_path,
            args.framework.as_deref(),
            args.control.as_deref(),
            args.format.as_deref(),
            args.recommend,
            args.limit,
        )
    })
    .map(ToolOutput::from))
}

/// Dispatch `tumult_compliance_lineage`: the (regulatory article, service)
/// lineage matrix with latest evidence verdicts. Reads the store read-only.
pub(super) fn compliance_lineage(params: &CallToolRequestParams) -> Dispatched {
    let args: ComplianceLineageTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::compliance_lineage(
            &args.store_path,
            args.framework.as_deref(),
            args.control.as_deref(),
            args.service.as_deref(),
        )
    })
    .map(ToolOutput::from))
}

/// Dispatch `tumult_recommend_injection`: rank the next most valuable fault
/// injections from lineage, topology, and the plugin catalog. Read-only.
pub(super) fn recommend_injection(params: &CallToolRequestParams) -> Dispatched {
    let args: RecommendInjectionTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::recommend_injection(&args.store_path, args.framework.as_deref(), args.limit)
    })
    .map(ToolOutput::from))
}
