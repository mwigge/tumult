//! Dispatch bodies for analytics and query tools: `analyze`, `store_stats`,
//! `analyze_store`, `compliance`, `trend`, `coverage`, and `recommend`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::handler::schema::{
    AnalyzeStoreTool, AnalyzeTool, ComplianceTool, CoverageTool, RecommendTool, StoreStatsTool,
    TrendTool,
};
use crate::handler::TumultHandler;
use crate::tools;

use super::{parse_args, Dispatched, ToolOutput};

pub(super) fn analyze(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: AnalyzeTool = parse_args(params)?;
    let path = handler.resolve_path(&args.journals_path)?;
    Ok(tokio::task::block_in_place(|| tools::analyze(&path, &args.query)).map(ToolOutput::from))
}

pub(super) fn store_stats(params: &CallToolRequestParams) -> Dispatched {
    let args: StoreStatsTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| tools::store_stats(&args.store_path)).map(ToolOutput::from))
}

pub(super) fn analyze_store(params: &CallToolRequestParams) -> Dispatched {
    let args: AnalyzeStoreTool = parse_args(params)?;
    Ok(
        tokio::task::block_in_place(|| tools::analyze_persistent(&args.store_path, &args.query))
            .map(ToolOutput::from),
    )
}

pub(super) fn compliance(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: ComplianceTool = parse_args(params)?;
    let path = handler.resolve_path(&args.journals_path)?;
    Ok(
        tokio::task::block_in_place(|| tools::compliance(&path, &args.framework))
            .map(ToolOutput::from),
    )
}

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

pub(super) fn coverage(params: &CallToolRequestParams) -> Dispatched {
    let args: CoverageTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| tools::coverage(&args.store_path)).map(ToolOutput::from))
}

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
