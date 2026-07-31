//! Dispatch bodies for autopilot tools: `autopilot_run`, `autopilot_status`,
//! `autopilot_respond`, and `autopilot_export`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::handler::schema::{
    AutopilotExportTool, AutopilotRespondTool, AutopilotRunTool, AutopilotStatusTool,
};
use crate::handler::{Role, TumultHandler};
use crate::tools;

use super::{parse_args, store_path_for, Dispatched, ToolOutput};

/// Dispatch `tumult_autopilot_run`: run one autopilot decision-loop pass over
/// a policy TOML. With `execute=true` this is an enact path and must hold the
/// server-wide `EnactLock` slot; without the slot the gate evaluation reads
/// the in-flight count so concurrent enactments are vetoed, never queued.
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

/// Dispatch `tumult_autopilot_status`: list recorded autopilot decisions with
/// their latest lifecycle event. Viewer-role callers are pinned to the
/// default store path (see `store_path_for`).
pub(super) fn autopilot_status(params: &CallToolRequestParams, role: Option<Role>) -> Dispatched {
    let args: AutopilotStatusTool = parse_args(params)?;
    let store_path = store_path_for(role, &args.store_path);
    Ok(tokio::task::block_in_place(|| {
        tools::autopilot_status(&store_path, args.verdict.as_deref(), args.limit)
    })
    .map(ToolOutput::from))
}

/// Dispatch `tumult_autopilot_respond`: record the human response to a
/// proposed/downgraded decision. `approve=true` is an enact path and takes
/// the same `EnactLock` slot as `autopilot_run`, so a stale approval is
/// re-gated against current state and vetoed while another enactment runs.
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

/// Dispatch `tumult_autopilot_export`: export the autopilot decision and
/// event tables as a Parquet archive into the given directory.
pub(super) fn autopilot_export(params: &CallToolRequestParams) -> Dispatched {
    let args: AutopilotExportTool = parse_args(params)?;
    Ok(
        tokio::task::block_in_place(|| tools::autopilot_export(&args.store_path, &args.dir))
            .map(ToolOutput::from),
    )
}

/// Dispatch `tumult_autopilot_notify`: record an external change event
/// against a service so the next pass proposes revalidation. Insert-only.
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
