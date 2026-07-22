//! Dispatch bodies for autopilot tools: `autopilot_run`, `autopilot_status`,
//! `autopilot_respond`, and `autopilot_export`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::handler::schema::{
    AutopilotExportTool, AutopilotRespondTool, AutopilotRunTool, AutopilotStatusTool,
};
use crate::handler::{Role, TumultHandler};
use crate::tools;

use super::{parse_args, store_path_for, Dispatched, ToolOutput};

pub(super) fn autopilot_run(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: AutopilotRunTool = parse_args(params)?;
    let execute = args.execute.unwrap_or(false);
    // The enactment ledger: a pass that will run faults must hold the
    // server-wide slot, and its gate evaluation reads
    // `concurrent_experiments = 0` (it *is* the one allowed enactment). A
    // pass that cannot take the slot — or that does not execute at all —
    // gates against the in-flight count, so the
    // `ambient.no_concurrent_experiment` rule vetoes its enact verdicts.
    // The guard is RAII: released on completion and on error.
    let guard = if execute {
        handler.enact_lock.try_acquire()
    } else {
        None
    };
    let concurrent = if guard.is_some() {
        0
    } else {
        handler.enact_lock.in_flight()
    };
    let result = tokio::task::block_in_place(|| {
        tools::autopilot_once(
            &args.store_path,
            &args.policy_path,
            execute,
            args.limit,
            concurrent,
        )
    });
    drop(guard);
    Ok(result.map(ToolOutput::from))
}

pub(super) fn autopilot_status(params: &CallToolRequestParams, role: Option<Role>) -> Dispatched {
    let args: AutopilotStatusTool = parse_args(params)?;
    let store_path = store_path_for(role, &args.store_path);
    Ok(tokio::task::block_in_place(|| {
        tools::autopilot_status(&store_path, args.verdict.as_deref(), args.limit)
    })
    .map(ToolOutput::from))
}

pub(super) fn autopilot_respond(
    handler: &TumultHandler,
    params: &CallToolRequestParams,
) -> Dispatched {
    let args: AutopilotRespondTool = parse_args(params)?;
    // Same ledger as `autopilot_run`: an approval executes the playbook, so
    // it must hold the slot for its re-gate to read 0; without the slot the
    // re-gate sees the in-flight enactment and vetoes the stale approval.
    let guard = if args.approve {
        handler.enact_lock.try_acquire()
    } else {
        None
    };
    let concurrent = if guard.is_some() {
        0
    } else {
        handler.enact_lock.in_flight()
    };
    let result = tokio::task::block_in_place(|| {
        tools::autopilot_respond(
            &args.store_path,
            &args.decision_id,
            args.approve,
            args.reason.as_deref(),
            args.policy_path.as_deref(),
            concurrent,
        )
    });
    drop(guard);
    Ok(result.map(ToolOutput::from))
}

pub(super) fn autopilot_export(params: &CallToolRequestParams) -> Dispatched {
    let args: AutopilotExportTool = parse_args(params)?;
    Ok(
        tokio::task::block_in_place(|| tools::autopilot_export(&args.store_path, &args.dir))
            .map(ToolOutput::from),
    )
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
