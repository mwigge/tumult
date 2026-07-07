//! Dispatch bodies for autopilot tools: `autopilot_run`, `autopilot_status`,
//! `autopilot_respond`, and `autopilot_export`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::handler::schema::{
    AutopilotExportTool, AutopilotRespondTool, AutopilotRunTool, AutopilotStatusTool,
};
use crate::tools;

use super::{parse_args, Dispatched, ToolOutput};

pub(super) fn autopilot_run(params: &CallToolRequestParams) -> Dispatched {
    let args: AutopilotRunTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::autopilot_once(
            &args.store_path,
            &args.policy_path,
            args.execute.unwrap_or(false),
            args.limit,
        )
    })
    .map(ToolOutput::from))
}

pub(super) fn autopilot_status(params: &CallToolRequestParams) -> Dispatched {
    let args: AutopilotStatusTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::autopilot_status(&args.store_path, args.verdict.as_deref(), args.limit)
    })
    .map(ToolOutput::from))
}

pub(super) fn autopilot_respond(params: &CallToolRequestParams) -> Dispatched {
    let args: AutopilotRespondTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::autopilot_respond(
            &args.store_path,
            &args.decision_id,
            args.approve,
            args.reason.as_deref(),
        )
    })
    .map(ToolOutput::from))
}

pub(super) fn autopilot_export(params: &CallToolRequestParams) -> Dispatched {
    let args: AutopilotExportTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::autopilot_export(&args.store_path, &args.dir)
    })
    .map(ToolOutput::from))
}

pub(super) fn autopilot_notify(params: &CallToolRequestParams) -> Dispatched {
    let args: crate::handler::schema::AutopilotNotifyTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::autopilot_notify_change(
            &args.store_path,
            &args.service,
            &args.source,
            args.detail.as_deref(),
        )
    })
    .map(ToolOutput::from))
}
