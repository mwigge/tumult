//! Dispatch bodies for `GameDay` campaign tools: `gameday_create`,
//! `gameday_run`, `gameday_analyze`, and `gameday_list`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::handler::schema::{
    GameDayAnalyzeTool, GameDayCreateTool, GameDayListTool, GameDayRunTool,
};
use crate::handler::TumultHandler;
use crate::tools;

use super::{parse_args, validate_page, Dispatched, ToolOutput};

/// Dispatch `tumult_gameday_create`: write a `<name>.gameday.toon` campaign
/// file into the workspace root and link it as a `tumult://gameday/…` resource.
pub(super) fn gameday_create(
    handler: &TumultHandler,
    params: &CallToolRequestParams,
) -> Dispatched {
    let args: GameDayCreateTool = parse_args(params)?;
    let output_rel = format!("{}.gameday.toon", args.name);
    let output_path = handler.resolve_output_path(&output_rel)?;
    Ok(tokio::task::block_in_place(|| {
        tools::gameday_create(&tools::GameDayCreateRequest {
            output_path: std::path::Path::new(&output_path),
            name: &args.name,
            experiments: &args.experiments,
            load_tool: args.load_tool.as_deref(),
            load_script: args.load_script.as_deref(),
            load_vus: args.load_vus,
            framework: args.framework.as_deref(),
        })
    })
    .map(|report| {
        let link = crate::handler::resources::workspace_resource_link(
            &handler.workspace_root,
            crate::handler::resources::ResourceKind::Gameday,
            std::path::Path::new(&output_path),
        );
        ToolOutput::from(report).with_links(vec![link])
    }))
}

/// Dispatch `tumult_gameday_run`: execute every experiment in a `.gameday.toon`
/// campaign under shared load. An enact path: the whole campaign runs under one
/// `EnactLock` slot and is refused while another enactment is in flight.
pub(super) fn gameday_run(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: GameDayRunTool = parse_args(params)?;
    let path = handler.resolve_path(&args.gameday_path)?;
    // A campaign runs its experiments sequentially under one enactment slot
    // (see run_experiment); refuse while another enactment is in flight.
    let Some(guard) = handler.enact_lock.try_acquire() else {
        return Ok(Err(crate::error::ToolError::Execution(
            "another fault-injection enactment is already running on this server; retry when it \
             completes"
                .into(),
        )));
    };
    let result = tokio::task::block_in_place(|| tools::gameday_run(&path));
    drop(guard);
    Ok(result.map(ToolOutput::from))
}

/// Dispatch `tumult_gameday_analyze`: analyze a completed `GameDay` journal —
/// resilience score, per-experiment results, and compliance article mapping.
pub(super) fn gameday_analyze(
    handler: &TumultHandler,
    params: &CallToolRequestParams,
) -> Dispatched {
    let args: GameDayAnalyzeTool = parse_args(params)?;
    let path = handler.resolve_path(&args.gameday_path)?;
    Ok(tokio::task::block_in_place(|| tools::gameday_analyze(&path)).map(ToolOutput::from))
}

/// Dispatch `tumult_gameday_list`: list `.gameday.toon` campaign files in the
/// workspace (or a validated subdirectory), paginated.
pub(super) fn gameday_list(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: GameDayListTool = parse_args(params)?;
    let (limit, offset) = validate_page(args.limit, args.offset)?;
    let search_root = if let Some(ref p) = args.path {
        handler.resolve_path(p)?
    } else {
        handler.workspace_root_str()?
    };
    Ok(
        tokio::task::block_in_place(|| tools::gameday_list(&search_root, limit, offset))
            .map(ToolOutput::from),
    )
}
