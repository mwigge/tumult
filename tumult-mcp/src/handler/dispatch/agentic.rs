//! Dispatch bodies for agentic-adapter tools: `agents`,
//! `agentic_list_scenarios`, `agentic_smoke`, and `agentic_run_experiment`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::handler::schema::{
    AgenticListScenariosTool, AgenticRunExperimentTool, AgenticSmokeTool, AgentsTool,
};
use crate::tools;

use super::{parse_args, Dispatched, ToolOutput};

/// Dispatch `tumult_agents`: list agent CLI adapters with install, version,
/// and auth state. Bridges the sync implementation via `block_in_place`.
pub(super) fn agents(params: &CallToolRequestParams) -> Dispatched {
    let _args: AgentsTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        Ok(ToolOutput::from(tools::agents()))
    }))
}

/// Dispatch `tumult_agentic_list_scenarios`: list the deterministic agentic
/// fault-injection scenario packs (metadata only).
pub(super) fn agentic_list_scenarios(params: &CallToolRequestParams) -> Dispatched {
    let _args: AgenticListScenariosTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(tools::agentic_list_scenarios).map(ToolOutput::from))
}

/// Dispatch `tumult_agentic_smoke`: run a deterministic local agentic smoke
/// check against an adapter scenario (metadata only; no raw payloads).
pub(super) fn agentic_smoke(params: &CallToolRequestParams) -> Dispatched {
    let args: AgenticSmokeTool = parse_args(params)?;
    // Tool-surface span; the experiment span emitted inside nests
    // under it. The MCP transport hides the inbound traceparent, so
    // this is the correlate tier (tagged tumult.client=unknown).
    let tool = tumult_otel::agentic_span::start_tool_span(
        tumult_otel::agentic::TumultClient::Unknown.as_str(),
        "tumult_agentic_smoke",
    );
    let _guard = tool.context().clone().attach();
    let result = tokio::task::block_in_place(|| {
        tools::agentic_smoke(
            &args.adapter,
            &args.scenario,
            args.fault.as_deref(),
            args.contract.as_deref(),
        )
    });
    tool.end();
    Ok(result.map(ToolOutput::from))
}

/// Dispatch `tumult_agentic_run_experiment`: run a deterministic bundled
/// agentic scenario as a chaos experiment. Uses the same correlate-tier tool
/// span as `agentic_smoke` (the MCP transport hides the inbound traceparent).
pub(super) fn agentic_run_experiment(params: &CallToolRequestParams) -> Dispatched {
    let args: AgenticRunExperimentTool = parse_args(params)?;
    let tool = tumult_otel::agentic_span::start_tool_span(
        tumult_otel::agentic::TumultClient::Unknown.as_str(),
        "tumult_agentic_run_experiment",
    );
    let _guard = tool.context().clone().attach();
    let result = tokio::task::block_in_place(|| {
        tools::agentic_run_experiment(
            &args.adapter,
            &args.scenario,
            args.fault.as_deref(),
            args.contract.as_deref(),
        )
    });
    tool.end();
    Ok(result.map(ToolOutput::from))
}
