//! Dispatch bodies for agentic-adapter tools: `agents`,
//! `agentic_list_scenarios`, `agentic_smoke`, and `agentic_run_experiment`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::handler::schema::{
    AgenticListScenariosTool, AgenticRunExperimentTool, AgenticSmokeTool, AgentsTool,
};
use crate::tools;

use super::{parse_args, Dispatched, ToolOutput};

pub(super) fn agents(params: &CallToolRequestParams) -> Dispatched {
    let _args: AgentsTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        Ok(ToolOutput::from(tools::agents()))
    }))
}

pub(super) fn agentic_list_scenarios(params: &CallToolRequestParams) -> Dispatched {
    let _args: AgenticListScenariosTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(tools::agentic_list_scenarios).map(ToolOutput::from))
}

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
