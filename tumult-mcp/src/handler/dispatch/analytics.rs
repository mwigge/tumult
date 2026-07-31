//! Dispatch bodies for analytics and query tools: `analyze`, `store_stats`,
//! `analyze_store`, `compliance`, `trend`, `coverage`, and `recommend`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::handler::schema::{
    AnalyzeStoreTool, AnalyzeTool, ComplianceTool, CoverageTool, RecommendTool, StoreStatsTool,
    TrendTool,
};
use crate::handler::{Role, TumultHandler};
use crate::tools;

use super::{parse_args, store_path_for, Dispatched, ToolOutput};

/// Dispatch `tumult_analyze`: run a SQL query over experiment journals via
/// embedded `DuckDB`. The journals path is validated against the workspace root.
pub(super) fn analyze(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: AnalyzeTool = parse_args(params)?;
    let path = handler.resolve_path(&args.journals_path)?;
    Ok(tokio::task::block_in_place(|| tools::analyze(&path, &args.query)).map(ToolOutput::from))
}

/// Dispatch `tumult_store_stats`: return persistent analytics store
/// statistics. Viewer-role callers are pinned to the default store path
/// (see `store_path_for`).
pub(super) fn store_stats(params: &CallToolRequestParams, role: Option<Role>) -> Dispatched {
    let args: StoreStatsTool = parse_args(params)?;
    let store_path = store_path_for(role, &args.store_path);
    Ok(tokio::task::block_in_place(|| tools::store_stats(&store_path)).map(ToolOutput::from))
}

/// Dispatch `tumult_analyze_store`: run a SQL query over the persistent
/// analytics store. Viewer-role callers are pinned to the default store path.
pub(super) fn analyze_store(params: &CallToolRequestParams, role: Option<Role>) -> Dispatched {
    let args: AnalyzeStoreTool = parse_args(params)?;
    let store_path = store_path_for(role, &args.store_path);
    Ok(
        tokio::task::block_in_place(|| tools::analyze_persistent(&store_path, &args.query))
            .map(ToolOutput::from),
    )
}

/// Dispatch `tumult_compliance`: compute a regulatory compliance summary
/// (pass rate, recovery compliance, verdict) over journals for a framework.
pub(super) fn compliance(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: ComplianceTool = parse_args(params)?;
    let path = handler.resolve_path(&args.journals_path)?;
    Ok(
        tokio::task::block_in_place(|| tools::compliance(&path, &args.framework))
            .map(ToolOutput::from),
    )
}

/// Dispatch `tumult_trend`: compute a cross-run metric trend over journals,
/// with optional time window and target-title filter.
pub(super) fn trend(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: TrendTool = parse_args(params)?;
    let path = handler.resolve_path(&args.journals_path)?;
    Ok(tokio::task::block_in_place(|| {
        tools::trend(
            &path,
            &args.metric,
            args.last.as_deref(),
            args.target.as_deref(),
        )
    })
    .map(ToolOutput::from))
}

/// Dispatch `tumult_coverage`: report which plugins, actions, and targets
/// have been tested vs available, from the analytics store.
pub(super) fn coverage(params: &CallToolRequestParams) -> Dispatched {
    let args: CoverageTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| tools::coverage(&args.store_path)).map(ToolOutput::from))
}

/// Dispatch `tumult_recommend`: recommend what to test next via deterministic
/// heuristics, optionally enhanced by a local agent CLI adapter. The
/// `generate_experiments_dir` override is validated as an output path.
pub(super) fn recommend(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: RecommendTool = parse_args(params)?;
    let generate_dir = args
        .generate_experiments_dir
        .as_deref()
        .map(|p| handler.resolve_output_path(p))
        .transpose()?;
    Ok(tokio::task::block_in_place(|| {
        tools::recommend(&tools::RecommendRequest {
            store_path: &args.store_path,
            goal: args.goal.as_deref(),
            model: args.model.as_deref(),
            include_draft: args.include_draft,
            format: &args.format,
            agent: args.agent.as_deref(),
            agent_model: args.agent_model.as_deref(),
            agent_timeout_secs: args.agent_timeout_secs,
            generate_dir: generate_dir.as_deref().map(std::path::Path::new),
            workspace_root: &handler.workspace_root,
        })
    })
    .map(ToolOutput::from))
}
