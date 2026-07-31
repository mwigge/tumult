//! Dispatch bodies for journal and reporting tools: `read_journal`,
//! `list_journals`, `query_traces`, and `report`.

use rust_mcp_sdk::schema::CallToolRequestParams;

use crate::handler::schema::{ListJournalsTool, QueryTracesTool, ReadJournalTool, ReportTool};
use crate::handler::TumultHandler;
use crate::tools;

use super::{parse_args, validate_page, Dispatched, ToolOutput};

/// Dispatch `tumult_read_journal`: read a journal file as JSON or raw TOON
/// (or a compact summary), after validating the path against the workspace.
pub(super) fn read_journal(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: ReadJournalTool = parse_args(params)?;
    let path = handler.resolve_path(&args.journal_path)?;
    Ok(
        tokio::task::block_in_place(|| tools::read_journal(&path, &args.format, args.summary))
            .map(ToolOutput::from),
    )
}

/// Dispatch `tumult_list_journals`: list `.toon` journal files in a directory,
/// paginated; each listed journal is also linked as a `tumult://journal/…`
/// resource.
pub(super) fn list_journals(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: ListJournalsTool = parse_args(params)?;
    let (limit, offset) = validate_page(args.limit, args.offset)?;
    let path = handler.resolve_path(&args.path)?;
    Ok(
        tokio::task::block_in_place(|| tools::list_journals(&path, limit, offset)).map(|report| {
            let links = handler.journal_page_links(&report.structured);
            ToolOutput::from(report).with_links(links)
        }),
    )
}

/// Dispatch `tumult_query_traces`: extract activity spans with trace/span IDs
/// from a journal for observability correlation.
pub(super) fn query_traces(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: QueryTracesTool = parse_args(params)?;
    let path = handler.resolve_path(&args.journal_path)?;
    Ok(tokio::task::block_in_place(|| tools::query_traces(&path)).map(ToolOutput::from))
}

/// Dispatch `tumult_report`: render a journal as a report (JSON or `JUnit` XML).
/// With `output_path` the report is written inside the workspace and returned
/// as a resource link; otherwise the content is returned inline.
pub(super) fn report(handler: &TumultHandler, params: &CallToolRequestParams) -> Dispatched {
    let args: ReportTool = parse_args(params)?;
    let path = handler.resolve_path(&args.journal_path)?;
    let output_path = args
        .output_path
        .as_deref()
        .map(|p| handler.resolve_output_path(p))
        .transpose()?;
    Ok(tokio::task::block_in_place(|| {
        tools::report(
            &path,
            &args.format,
            output_path.as_deref().map(std::path::Path::new),
        )
    })
    .map(|report| {
        let mut output = ToolOutput::from(report);
        if let Some(ref out) = output_path {
            output
                .links
                .push(crate::handler::resources::file_resource_link(
                    std::path::Path::new(out),
                ));
        }
        output
    }))
}
