//! Dispatch bodies for experiment authoring and execution tools:
//! `run_experiment`, `validate`, `create_experiment`, `scaffold_experiment`,
//! and `list_experiments`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::handler::schema::{
    CreateExperimentTool, ListExperimentsTool, RunExperimentTool, ScaffoldExperimentTool,
    ValidateTool,
};
use crate::handler::TumultHandler;
use crate::tools;

use super::{parse_args, validate_page, Dispatched, ToolOutput, DEFAULT_JOURNAL_PATH};

/// Dispatch `tumult_run_experiment`: execute a chaos experiment and persist
/// its journal. An enact path: refuses (as a tool-level error, not a protocol
/// error) while another enactment holds the `EnactLock` slot — a queued fault
/// is a stale fault. The captured MCP span context is passed through as the
/// parent of the experiment trace.
pub(super) fn run_experiment(
    handler: &TumultHandler,
    params: &CallToolRequestParams,
    mcp_context: opentelemetry::Context,
) -> Dispatched {
    let args: RunExperimentTool = parse_args(params)?;
    let path = handler.resolve_path(&args.experiment_path)?;
    let journal_rel = args.journal_path.as_deref().unwrap_or(DEFAULT_JOURNAL_PATH);
    let journal_path = handler.resolve_output_path(journal_rel)?;
    // An enact path like any other: while another enactment holds the
    // server-wide slot, refuse rather than queue — a queued fault is a
    // stale fault. The RAII guard releases on completion and on error.
    let Some(guard) = handler.enact_lock.try_acquire() else {
        return Ok(Err(crate::error::ToolError::Execution(
            "another fault-injection enactment is already running on this server; retry when it \
             completes"
                .into(),
        )));
    };
    let result = tokio::task::block_in_place(|| {
        tools::run_experiment(tools::RunExperimentRequest {
            experiment_path: &path,
            rollback_strategy: &args.rollback_strategy,
            journal_path: std::path::Path::new(&journal_path),
            store_path: &args.store_path,
            no_ingest: args.no_ingest,
            format: &args.format,
            parent_context: Some(mcp_context),
        })
    });
    drop(guard);
    Ok(result.map(|report| {
        let journal = std::path::Path::new(&journal_path);
        let link = crate::handler::resources::workspace_resource_link(
            &handler.workspace_root,
            crate::handler::resources::classify(journal),
            journal,
        );
        ToolOutput::from(report).with_links(vec![link])
    }))
}

/// Dispatch `tumult_validate`: validate an experiment file for syntax and
/// provider support. Read-only.
pub(super) fn validate(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: ValidateTool = parse_args(params)?;
    let path = handler.resolve_path(&args.experiment_path)?;
    Ok(tokio::task::block_in_place(|| tools::validate_experiment(&path)).map(ToolOutput::from))
}

/// Dispatch `tumult_create_experiment`: scaffold a new experiment file from a
/// template. The output path is validated against the workspace root.
pub(super) fn create_experiment(
    handler: &TumultHandler,
    params: &CallToolRequestParams,
) -> Dispatched {
    let args: CreateExperimentTool = parse_args(params)?;
    let path = handler.resolve_output_path(&args.output_path)?;
    Ok(
        tokio::task::block_in_place(|| tools::create_experiment(&path, args.plugin.as_deref()))
            .map(ToolOutput::from),
    )
}

/// Dispatch `tumult_list_experiments`: list `.toon` experiment files under
/// the workspace (or a validated subdirectory), paginated.
pub(super) fn list_experiments(
    handler: &TumultHandler,
    params: &CallToolRequestParams,
) -> Dispatched {
    let args: ListExperimentsTool = parse_args(params)?;
    let (limit, offset) = validate_page(args.limit, args.offset)?;
    let search_root = if let Some(ref p) = args.path {
        handler.resolve_path(p)?
    } else {
        handler.workspace_root_str()?
    };
    Ok(
        tokio::task::block_in_place(|| tools::list_experiments(&search_root, limit, offset))
            .map(ToolOutput::from),
    )
}

/// Dispatch `tumult_scaffold_experiment`: generate a validated experiment
/// from a chosen fault action. Pure generation — read-only w.r.t. the store.
pub(super) fn scaffold_experiment(params: &CallToolRequestParams) -> Dispatched {
    let args: ScaffoldExperimentTool = parse_args(params)?;
    Ok(tokio::task::block_in_place(|| {
        tools::scaffold_experiment(&tools::ScaffoldArgs {
            plugin: args.plugin.as_deref(),
            action: &args.action,
            args: &args.args,
            target: &args.target,
            probe_command: args.probe_command.as_deref(),
            probe_url: args.probe_url.as_deref(),
            probe_expect: args.probe_expect.as_deref(),
            title: args.title.as_deref(),
        })
        .map(ToolOutput::from)
    }))
}
