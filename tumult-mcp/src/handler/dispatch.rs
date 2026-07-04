//! `ServerHandler` implementation — lists tools and dispatches tool calls.

use std::sync::Arc;

use async_trait::async_trait;
use rust_mcp_sdk::{
    mcp_server::ServerHandler,
    schema::{
        CallToolError, CallToolRequestParams, CallToolResult, ContentBlock, ListResourcesResult,
        ListToolsResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResult,
        ResourceLink, RpcError,
    },
    McpServer,
};

use crate::tools;

use super::output_schema::output_schema_for;
use super::schema::{
    AgenticListScenariosTool, AgenticRunExperimentTool, AgenticSmokeTool, AgentsTool,
    AnalyzeStoreTool, AnalyzeTool, ComplianceTool, CoverageTool, CreateExperimentTool,
    DiscoverTool, GameDayAnalyzeTool, GameDayCreateTool, GameDayListTool, GameDayRunTool,
    ListExperimentsTool, ListJournalsTool, QueryTracesTool, ReadJournalTool, RecommendTool,
    ReportTool, RunExperimentTool, StoreStatsTool, TrendTool, ValidateTool,
};
use super::TumultHandler;

/// Journal file name used when `tumult_run_experiment` receives no
/// `journal_path` — mirrors the CLI's `--journal-path` default.
const DEFAULT_JOURNAL_PATH: &str = "journal.toon";

/// Maximum accepted value for the list tools' `limit` parameter.
const LIST_LIMIT_MAX: u64 = 1000;

/// Internal result of a dispatched tool call: text content plus the
/// optional structured-content object and any `resource_link` content
/// items appended after the text.
struct ToolOutput {
    text: String,
    structured: Option<serde_json::Map<String, serde_json::Value>>,
    links: Vec<ResourceLink>,
}

impl ToolOutput {
    /// Attach `resource_link` content items to this output.
    fn with_links(mut self, links: Vec<ResourceLink>) -> Self {
        self.links = links;
        self
    }
}

impl From<String> for ToolOutput {
    fn from(text: String) -> Self {
        Self {
            text,
            structured: None,
            links: Vec::new(),
        }
    }
}

impl From<tools::StructuredReport> for ToolOutput {
    fn from(report: tools::StructuredReport) -> Self {
        Self {
            text: report.text,
            structured: Some(report.structured),
            links: Vec::new(),
        }
    }
}

/// Validate the list tools' `limit`/`offset` parameters and convert them
/// to `usize` (limit defaults to 100 via serde, max [`LIST_LIMIT_MAX`]).
fn validate_page(limit: u64, offset: u64) -> std::result::Result<(usize, usize), CallToolError> {
    if limit > LIST_LIMIT_MAX {
        return Err(CallToolError::invalid_arguments(
            "limit",
            Some(format!(
                "limit {limit} exceeds the maximum of {LIST_LIMIT_MAX}"
            )),
        ));
    }
    let limit = usize::try_from(limit)
        .map_err(|_| CallToolError::invalid_arguments("limit", Some("limit too large".into())))?;
    let offset = usize::try_from(offset)
        .map_err(|_| CallToolError::invalid_arguments("offset", Some("offset too large".into())))?;
    Ok((limit, offset))
}

impl TumultHandler {
    /// Validate and resolve a user-supplied *output* path against the
    /// workspace root. Unlike `resolve_path`, the leaf file may not exist
    /// yet — only the parent directory is canonicalized and containment
    /// checked (see `tools::safe_resolve_output_path`).
    ///
    /// # Errors
    ///
    /// Returns `CallToolError` if the path escapes the workspace root or
    /// the resolved path contains non-UTF-8 characters.
    fn resolve_output_path(&self, user_path: &str) -> std::result::Result<String, CallToolError> {
        let resolved = tools::safe_resolve_output_path(&self.workspace_root, user_path)
            .map_err(|e| CallToolError::invalid_arguments("path", Some(e.to_string())))?;
        resolved
            .to_str()
            .map(std::string::ToString::to_string)
            .ok_or_else(|| {
                CallToolError::invalid_arguments(
                    "path",
                    Some(format!(
                        "path contains non-UTF-8 characters: {}",
                        resolved.display()
                    )),
                )
            })
    }
}

#[async_trait]
impl ServerHandler for TumultHandler {
    async fn handle_list_tools_request(
        &self,
        _params: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ListToolsResult, RpcError> {
        let mut tools = vec![
            RunExperimentTool::tool(),
            ValidateTool::tool(),
            AnalyzeTool::tool(),
            ReadJournalTool::tool(),
            ListJournalsTool::tool(),
            DiscoverTool::tool(),
            CreateExperimentTool::tool(),
            QueryTracesTool::tool(),
            StoreStatsTool::tool(),
            AnalyzeStoreTool::tool(),
            ListExperimentsTool::tool(),
            ReportTool::tool(),
            ComplianceTool::tool(),
            TrendTool::tool(),
            AgentsTool::tool(),
            GameDayCreateTool::tool(),
            GameDayRunTool::tool(),
            GameDayAnalyzeTool::tool(),
            GameDayListTool::tool(),
            RecommendTool::tool(),
            CoverageTool::tool(),
            AgenticListScenariosTool::tool(),
            AgenticSmokeTool::tool(),
            AgenticRunExperimentTool::tool(),
        ];
        // The mcp_tool macro hardcodes output_schema to None; patch in the
        // hand-written schemas for tools that return structured content.
        for tool in &mut tools {
            tool.output_schema = output_schema_for(&tool.name);
        }
        Ok(ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    #[allow(clippy::too_many_lines)] // Tool dispatch requires one match arm per tool; extracting to closures would not reduce the logical complexity
    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        // Acquire rate-limiting permit before any non-Send work
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| CallToolError::from_message("semaphore closed".to_string()))?;

        // Enforce bearer token authentication if configured.
        // Clients pass the Authorization value via `_meta.authorization` since
        // stdio transport has no HTTP header context at the handler level.
        let authorization = Self::extract_authorization(&params);
        if let Err(e) = self.auth.check(authorization.as_deref()) {
            return Err(CallToolError::from_message(format!("Unauthorized: {e}")));
        }

        tracing::info!(tool = %params.name, "MCP tool call");
        // SpanGuard contains a non-Send OTel context guard. Capture the active
        // context while the span is alive, then drop the guard so the future
        // remains Send. The captured context is passed to run_experiment as
        // parent_context so the resilience.experiment span is linked here.
        let mcp_context = {
            let _span = crate::telemetry::begin_tool_call(&params.name);
            crate::telemetry::current_context()
        };

        let result = match params.name.as_str() {
            "tumult_run_experiment" => {
                let args: RunExperimentTool = parse_args(&params)?;
                let path = self.resolve_path(&args.experiment_path)?;
                let journal_rel = args.journal_path.as_deref().unwrap_or(DEFAULT_JOURNAL_PATH);
                let journal_path = self.resolve_output_path(journal_rel)?;
                tokio::task::block_in_place(|| {
                    tools::run_experiment(tools::RunExperimentRequest {
                        experiment_path: &path,
                        rollback_strategy: &args.rollback_strategy,
                        journal_path: std::path::Path::new(&journal_path),
                        store_path: &args.store_path,
                        no_ingest: args.no_ingest,
                        format: &args.format,
                        parent_context: Some(mcp_context),
                    })
                })
                .map(|report| {
                    let journal = std::path::Path::new(&journal_path);
                    let link = super::resources::workspace_resource_link(
                        &self.workspace_root,
                        super::resources::classify(journal),
                        journal,
                    );
                    ToolOutput::from(report).with_links(vec![link])
                })
            }
            "tumult_validate" => {
                let args: ValidateTool = parse_args(&params)?;
                let path = self.resolve_path(&args.experiment_path)?;
                tokio::task::block_in_place(|| tools::validate_experiment(&path))
                    .map(ToolOutput::from)
            }
            "tumult_analyze" => {
                let args: AnalyzeTool = parse_args(&params)?;
                let path = self.resolve_path(&args.journals_path)?;
                tokio::task::block_in_place(|| tools::analyze(&path, &args.query))
                    .map(ToolOutput::from)
            }
            "tumult_read_journal" => {
                let args: ReadJournalTool = parse_args(&params)?;
                let path = self.resolve_path(&args.journal_path)?;
                tokio::task::block_in_place(|| {
                    tools::read_journal(&path, &args.format, args.summary)
                })
                .map(ToolOutput::from)
            }
            "tumult_list_journals" => {
                let args: ListJournalsTool = parse_args(&params)?;
                let (limit, offset) = validate_page(args.limit, args.offset)?;
                let path = self.resolve_path(&args.path)?;
                tokio::task::block_in_place(|| tools::list_journals(&path, limit, offset)).map(
                    |report| {
                        let links = self.journal_page_links(&report.structured);
                        ToolOutput::from(report).with_links(links)
                    },
                )
            }
            "tumult_discover" => {
                tokio::task::block_in_place(|| Ok(ToolOutput::from(tools::discover_plugins())))
            }
            "tumult_create_experiment" => {
                let args: CreateExperimentTool = parse_args(&params)?;
                let path = self.resolve_output_path(&args.output_path)?;
                tokio::task::block_in_place(|| {
                    tools::create_experiment(&path, args.plugin.as_deref())
                })
                .map(ToolOutput::from)
            }
            "tumult_query_traces" => {
                let args: QueryTracesTool = parse_args(&params)?;
                let path = self.resolve_path(&args.journal_path)?;
                tokio::task::block_in_place(|| tools::query_traces(&path)).map(ToolOutput::from)
            }
            "tumult_store_stats" => {
                let args: StoreStatsTool = parse_args(&params)?;
                tokio::task::block_in_place(|| tools::store_stats(&args.store_path))
                    .map(ToolOutput::from)
            }
            "tumult_analyze_store" => {
                let args: AnalyzeStoreTool = parse_args(&params)?;
                tokio::task::block_in_place(|| {
                    tools::analyze_persistent(&args.store_path, &args.query)
                })
                .map(ToolOutput::from)
            }
            "tumult_list_experiments" => {
                let args: ListExperimentsTool = parse_args(&params)?;
                let (limit, offset) = validate_page(args.limit, args.offset)?;
                let search_root = if let Some(ref p) = args.path {
                    self.resolve_path(p)?
                } else {
                    self.workspace_root_str()?
                };
                tokio::task::block_in_place(|| tools::list_experiments(&search_root, limit, offset))
                    .map(ToolOutput::from)
            }
            "tumult_report" => {
                let args: ReportTool = parse_args(&params)?;
                let path = self.resolve_path(&args.journal_path)?;
                let output_path = args
                    .output_path
                    .as_deref()
                    .map(|p| self.resolve_output_path(p))
                    .transpose()?;
                tokio::task::block_in_place(|| {
                    tools::report(
                        &path,
                        &args.format,
                        output_path.as_deref().map(std::path::Path::new),
                    )
                })
                .map(|report| {
                    let mut output = ToolOutput::from(report);
                    if let Some(ref out) = output_path {
                        output.links.push(super::resources::file_resource_link(
                            std::path::Path::new(out),
                        ));
                    }
                    output
                })
            }
            "tumult_compliance" => {
                let args: ComplianceTool = parse_args(&params)?;
                let path = self.resolve_path(&args.journals_path)?;
                tokio::task::block_in_place(|| tools::compliance(&path, &args.framework))
                    .map(ToolOutput::from)
            }
            "tumult_trend" => {
                let args: TrendTool = parse_args(&params)?;
                let path = self.resolve_path(&args.journals_path)?;
                tokio::task::block_in_place(|| {
                    tools::trend(
                        &path,
                        &args.metric,
                        args.last.as_deref(),
                        args.target.as_deref(),
                    )
                })
                .map(ToolOutput::from)
            }
            "tumult_agents" => {
                let _args: AgentsTool = parse_args(&params)?;
                tokio::task::block_in_place(|| Ok(ToolOutput::from(tools::agents())))
            }
            "tumult_gameday_create" => {
                let args: GameDayCreateTool = parse_args(&params)?;
                let output_rel = format!("{}.gameday.toon", args.name);
                let output_path = self.resolve_output_path(&output_rel)?;
                tokio::task::block_in_place(|| {
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
                    let link = super::resources::workspace_resource_link(
                        &self.workspace_root,
                        super::resources::ResourceKind::Gameday,
                        std::path::Path::new(&output_path),
                    );
                    ToolOutput::from(report).with_links(vec![link])
                })
            }
            "tumult_gameday_run" => {
                let args: GameDayRunTool = parse_args(&params)?;
                let path = self.resolve_path(&args.gameday_path)?;
                tokio::task::block_in_place(|| tools::gameday_run(&path)).map(ToolOutput::from)
            }
            "tumult_gameday_analyze" => {
                let args: GameDayAnalyzeTool = parse_args(&params)?;
                let path = self.resolve_path(&args.gameday_path)?;
                tokio::task::block_in_place(|| tools::gameday_analyze(&path)).map(ToolOutput::from)
            }
            "tumult_gameday_list" => {
                let args: GameDayListTool = parse_args(&params)?;
                let (limit, offset) = validate_page(args.limit, args.offset)?;
                let search_root = if let Some(ref p) = args.path {
                    self.resolve_path(p)?
                } else {
                    self.workspace_root_str()?
                };
                tokio::task::block_in_place(|| tools::gameday_list(&search_root, limit, offset))
                    .map(ToolOutput::from)
            }
            "tumult_recommend" => {
                let args: RecommendTool = parse_args(&params)?;
                let generate_dir = args
                    .generate_experiments_dir
                    .as_deref()
                    .map(|p| self.resolve_output_path(p))
                    .transpose()?;
                tokio::task::block_in_place(|| {
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
                        workspace_root: &self.workspace_root,
                    })
                })
                .map(ToolOutput::from)
            }
            "tumult_coverage" => {
                let args: CoverageTool = parse_args(&params)?;
                tokio::task::block_in_place(|| tools::coverage(&args.store_path))
                    .map(ToolOutput::from)
            }
            "tumult_agentic_list_scenarios" => {
                let _args: AgenticListScenariosTool = parse_args(&params)?;
                tokio::task::block_in_place(tools::agentic_list_scenarios).map(ToolOutput::from)
            }
            "tumult_agentic_smoke" => {
                let args: AgenticSmokeTool = parse_args(&params)?;
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
                result.map(ToolOutput::from)
            }
            "tumult_agentic_run_experiment" => {
                let args: AgenticRunExperimentTool = parse_args(&params)?;
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
                result.map(ToolOutput::from)
            }
            _ => return Err(CallToolError::unknown_tool(params.name)),
        };

        finish_tool_call(&params.name, result)
    }

    async fn handle_list_resources_request(
        &self,
        params: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ListResourcesResult, RpcError> {
        let params = params.unwrap_or_default();
        // Resources go through the same bearer gate as tool calls.
        self.check_resource_auth(params.meta.as_ref().and_then(|m| m.extra.as_ref()))?;
        tokio::task::block_in_place(|| self.list_resources_page(params.cursor.as_deref()))
    }

    async fn handle_read_resource_request(
        &self,
        params: ReadResourceRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ReadResourceResult, RpcError> {
        self.check_resource_auth(params.meta.as_ref().and_then(|m| m.extra.as_ref()))?;
        tokio::task::block_in_place(|| self.read_resource_uri(&params.uri))
    }
}

/// Convert a dispatched tool outcome into a `CallToolResult`, emitting the
/// completion/error telemetry event.
fn finish_tool_call(
    tool_name: &str,
    result: std::result::Result<ToolOutput, crate::error::ToolError>,
) -> std::result::Result<CallToolResult, CallToolError> {
    match result {
        Ok(output) => {
            crate::telemetry::event_tool_completed(tool_name, true);
            let mut result = CallToolResult::text_content(vec![output.text.into()]);
            result.structured_content = output.structured;
            // resource_link content items follow the text block so
            // text-only clients keep working unchanged.
            result
                .content
                .extend(output.links.into_iter().map(ContentBlock::ResourceLink));
            Ok(result)
        }
        Err(e) => {
            crate::telemetry::event_tool_error(tool_name, &e.to_string());
            // Per MCP spec, tool-level failures are reported inside the
            // result with `isError: true`, not as protocol errors.
            let mut result = CallToolResult::text_content(vec![format!("Error: {e}").into()]);
            result.is_error = Some(true);
            Ok(result)
        }
    }
}

fn parse_args<T: serde::de::DeserializeOwned>(
    params: &CallToolRequestParams,
) -> std::result::Result<T, CallToolError> {
    let value = serde_json::to_value(&params.arguments).unwrap_or_default();
    serde_json::from_value(value)
        .map_err(|e| CallToolError::invalid_arguments("parse_args", Some(e.to_string())))
}

#[cfg(test)]
// `vec![…]` is used to build expected tool name lists inline; the verbosity aids readability in tests.
#[allow(clippy::useless_vec)]
mod tests {
    use super::*;
    use crate::handler::{McpAuth, TumultHandler, MAX_CONCURRENT_TOOL_CALLS};

    #[test]
    fn all_tools_listed() {
        let tools = vec![
            RunExperimentTool::tool(),
            ValidateTool::tool(),
            AnalyzeTool::tool(),
            ReadJournalTool::tool(),
            ListJournalsTool::tool(),
            DiscoverTool::tool(),
            CreateExperimentTool::tool(),
            QueryTracesTool::tool(),
            StoreStatsTool::tool(),
            AnalyzeStoreTool::tool(),
            ListExperimentsTool::tool(),
            ReportTool::tool(),
            ComplianceTool::tool(),
            TrendTool::tool(),
            AgentsTool::tool(),
            GameDayCreateTool::tool(),
            GameDayRunTool::tool(),
            GameDayAnalyzeTool::tool(),
            GameDayListTool::tool(),
            RecommendTool::tool(),
            CoverageTool::tool(),
            AgenticListScenariosTool::tool(),
            AgenticSmokeTool::tool(),
            AgenticRunExperimentTool::tool(),
        ];
        assert_eq!(tools.len(), 24);
    }

    #[test]
    fn handler_has_semaphore_with_correct_limit() {
        let handler = TumultHandler::with_auth(
            std::env::current_dir().unwrap_or_else(|_| "/".into()),
            McpAuth { token: None },
        );
        assert_eq!(
            handler.semaphore.available_permits(),
            MAX_CONCURRENT_TOOL_CALLS
        );
    }

    #[test]
    fn tool_names_follow_convention() {
        let tools = [
            RunExperimentTool::tool(),
            ValidateTool::tool(),
            AnalyzeTool::tool(),
            ReadJournalTool::tool(),
            ListJournalsTool::tool(),
            DiscoverTool::tool(),
            CreateExperimentTool::tool(),
            QueryTracesTool::tool(),
            StoreStatsTool::tool(),
            AnalyzeStoreTool::tool(),
            ListExperimentsTool::tool(),
            ReportTool::tool(),
            ComplianceTool::tool(),
            TrendTool::tool(),
            AgentsTool::tool(),
            GameDayCreateTool::tool(),
            GameDayRunTool::tool(),
            GameDayAnalyzeTool::tool(),
            GameDayListTool::tool(),
            RecommendTool::tool(),
            CoverageTool::tool(),
            AgenticListScenariosTool::tool(),
            AgenticSmokeTool::tool(),
            AgenticRunExperimentTool::tool(),
        ];
        for tool in &tools {
            assert!(
                tool.name.starts_with("tumult_"),
                "tool name '{}' must start with tumult_",
                tool.name
            );
        }
    }

    /// Verify that the handler struct carries an `auth` field.
    /// This ensures authentication is structurally wired in, not just declared.
    #[test]
    fn auth_wired_into_handler() {
        // A handler with a configured token must carry it in the auth field.
        let handler = TumultHandler::with_auth(
            "/tmp".into(),
            McpAuth {
                token: Some("handler-secret".into()),
            },
        );
        // Auth check without token should fail (token is set on handler).
        assert!(handler.auth.check(None).is_err());
        // Auth check with correct bearer should pass.
        assert!(handler.auth.check(Some("Bearer handler-secret")).is_ok());
    }

    /// Verify the handler accepts requests when the correct bearer token is supplied.
    #[test]
    fn auth_wired_accepts_valid_token() {
        let handler = TumultHandler::with_auth(
            "/tmp".into(),
            McpAuth {
                token: Some("valid-token-xyz".into()),
            },
        );
        assert!(handler.auth.check(Some("Bearer valid-token-xyz")).is_ok());
        assert!(handler.auth.check(Some("Bearer wrong")).is_err());
    }

    #[test]
    fn handler_with_workspace_root_sets_path() {
        let handler = TumultHandler::with_workspace_root("/tmp".into());
        assert_eq!(handler.workspace_root, std::path::PathBuf::from("/tmp"));
    }

    #[test]
    fn workspace_root_str_returns_valid_path_for_utf8_root() {
        let handler = TumultHandler::with_workspace_root("/tmp".into());
        let result = handler.workspace_root_str();
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/tmp");
    }

    #[test]
    fn resolve_path_returns_error_for_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = TumultHandler::with_workspace_root(tmp.path().to_path_buf());
        let result = handler.resolve_path("../../etc/passwd");
        assert!(result.is_err(), "path traversal must be rejected");
    }

    // ── ServerHandler round trips (dispatch) ─────────────────────
    //
    // These tests invoke `handle_list_tools_request` / `handle_call_tool_request`
    // directly against a stub `McpServer` runtime — no transport, no network.

    use rust_mcp_sdk::schema::CallToolMeta;

    use crate::handler::test_support::stub_runtime;

    /// Build `CallToolRequestParams` from a tool name, JSON arguments, and an
    /// optional `_meta.authorization` value (how stdio clients pass the bearer).
    fn call_params(
        name: &str,
        arguments: serde_json::Value,
        authorization: Option<&str>,
    ) -> CallToolRequestParams {
        let arguments = match arguments {
            serde_json::Value::Object(map) => Some(map),
            _ => None,
        };
        let meta = authorization.map(|auth| {
            let mut extra = serde_json::Map::new();
            extra.insert(
                "authorization".into(),
                serde_json::Value::String(auth.into()),
            );
            CallToolMeta {
                progress_token: None,
                extra: Some(extra),
            }
        });
        CallToolRequestParams {
            name: name.into(),
            arguments,
            meta,
            task: None,
        }
    }

    /// Concatenate all text content blocks of a `CallToolResult`
    /// (`resource_link` blocks may follow the text).
    fn result_text(result: &CallToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::TextContent(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Collect the `resource_link` content blocks of a `CallToolResult`.
    fn result_links(result: &CallToolResult) -> Vec<&ResourceLink> {
        result
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ResourceLink(link) => Some(link),
                _ => None,
            })
            .collect()
    }

    /// Handler with no auth token, rooted at the given directory.
    fn open_handler(root: &std::path::Path) -> TumultHandler {
        TumultHandler::with_auth(root.to_path_buf(), McpAuth { token: None })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_tools_round_trip_returns_all_registered_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());
        let result = handler
            .handle_list_tools_request(None, stub_runtime())
            .await
            .expect("list_tools must succeed");

        // Every name dispatched in handle_call_tool_request must be listed.
        let expected = [
            "tumult_run_experiment",
            "tumult_validate",
            "tumult_analyze",
            "tumult_read_journal",
            "tumult_list_journals",
            "tumult_discover",
            "tumult_create_experiment",
            "tumult_query_traces",
            "tumult_store_stats",
            "tumult_analyze_store",
            "tumult_list_experiments",
            "tumult_report",
            "tumult_compliance",
            "tumult_trend",
            "tumult_agents",
            "tumult_gameday_create",
            "tumult_gameday_run",
            "tumult_gameday_analyze",
            "tumult_gameday_list",
            "tumult_recommend",
            "tumult_coverage",
            "tumult_agentic_list_scenarios",
            "tumult_agentic_smoke",
            "tumult_agentic_run_experiment",
        ];
        let names: Vec<&str> = result.tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names.len(),
            expected.len(),
            "listed tool count must match dispatch arms: {names:?}"
        );
        for name in expected {
            assert!(names.contains(&name), "tool '{name}' missing from list");
        }
        let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), names.len(), "tool names must be unique");
        assert!(result.next_cursor.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_tools_every_tool_has_description_and_object_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());
        let result = handler
            .handle_list_tools_request(None, stub_runtime())
            .await
            .expect("list_tools must succeed");

        for tool in &result.tools {
            assert!(
                tool.description.as_deref().is_some_and(|d| !d.is_empty()),
                "tool '{}' must have a non-empty description",
                tool.name
            );
            let schema = serde_json::to_value(&tool.input_schema)
                .unwrap_or_else(|e| panic!("schema for '{}' must serialize: {e}", tool.name));
            assert_eq!(
                schema.get("type").and_then(serde_json::Value::as_str),
                Some("object"),
                "input schema for '{}' must be a JSON object schema: {schema}",
                tool.name
            );
        }
    }

    /// Assert `structured` conforms to the schema advertised for `tool_name`:
    /// every required property is present and no undeclared keys appear.
    fn assert_conforms(tool_name: &str, structured: &serde_json::Map<String, serde_json::Value>) {
        let schema = output_schema_for(tool_name)
            .unwrap_or_else(|| panic!("'{tool_name}' must advertise an output schema"));
        let properties = schema.properties.clone().unwrap_or_default();
        for required in &schema.required {
            assert!(
                structured.contains_key(required),
                "'{tool_name}' structured content missing required property '{required}'"
            );
        }
        for key in structured.keys() {
            assert!(
                properties.contains_key(key),
                "'{tool_name}' structured content has undeclared property '{key}'"
            );
        }
    }

    #[test]
    fn tool_annotations_reflect_tool_behavior() {
        // Read-only, idempotent, closed-world tools. The agentic tools are
        // deterministic in-memory simulations (fake adapters only), so they
        // are classified read-only despite their "run" names.
        let read_only = [
            ValidateTool::tool(),
            AnalyzeTool::tool(),
            ReadJournalTool::tool(),
            ListJournalsTool::tool(),
            DiscoverTool::tool(),
            QueryTracesTool::tool(),
            StoreStatsTool::tool(),
            AnalyzeStoreTool::tool(),
            ListExperimentsTool::tool(),
            ComplianceTool::tool(),
            TrendTool::tool(),
            AgentsTool::tool(),
            GameDayAnalyzeTool::tool(),
            GameDayListTool::tool(),
            CoverageTool::tool(),
            AgenticListScenariosTool::tool(),
            AgenticSmokeTool::tool(),
            AgenticRunExperimentTool::tool(),
        ];
        for tool in &read_only {
            let a = tool
                .annotations
                .as_ref()
                .unwrap_or_else(|| panic!("'{}' must carry annotations", tool.name));
            assert_eq!(a.read_only_hint, Some(true), "read_only: {}", tool.name);
            assert_eq!(a.idempotent_hint, Some(true), "idempotent: {}", tool.name);
            assert_eq!(a.open_world_hint, Some(false), "open_world: {}", tool.name);
        }

        // Chaos executors: destructive and open-world.
        for tool in &[RunExperimentTool::tool(), GameDayRunTool::tool()] {
            let a = tool
                .annotations
                .as_ref()
                .unwrap_or_else(|| panic!("'{}' must carry annotations", tool.name));
            assert_eq!(a.destructive_hint, Some(true), "destructive: {}", tool.name);
            assert_eq!(a.read_only_hint, Some(false), "read_only: {}", tool.name);
            assert_eq!(a.idempotent_hint, Some(false), "idempotent: {}", tool.name);
            assert_eq!(a.open_world_hint, Some(true), "open_world: {}", tool.name);
        }

        // create_experiment and gameday_create write a new local file:
        // additive, not idempotent (second call errors), closed-world.
        for tool in &[CreateExperimentTool::tool(), GameDayCreateTool::tool()] {
            let a = tool
                .annotations
                .as_ref()
                .unwrap_or_else(|| panic!("'{}' must carry annotations", tool.name));
            assert_eq!(
                a.destructive_hint,
                Some(false),
                "destructive: {}",
                tool.name
            );
            assert_eq!(a.read_only_hint, Some(false), "read_only: {}", tool.name);
            assert_eq!(a.idempotent_hint, Some(false), "idempotent: {}", tool.name);
            assert_eq!(a.open_world_hint, Some(false), "open_world: {}", tool.name);
        }

        // report may write a file (output_path) but re-rendering the same
        // journal is idempotent and closed-world. Annotations are static, so
        // the write-capable classification wins over the inline-only path.
        let report = ReportTool::tool();
        let a = report.annotations.as_ref().expect("report annotations");
        assert_eq!(a.destructive_hint, Some(false));
        assert_eq!(a.read_only_hint, Some(false));
        assert_eq!(a.idempotent_hint, Some(true));
        assert_eq!(a.open_world_hint, Some(false));

        // recommend can spawn a local agent CLI (which may reach its model
        // API over the network) and can write validated experiment files.
        let recommend = RecommendTool::tool();
        let a = recommend
            .annotations
            .as_ref()
            .expect("recommend annotations");
        assert_eq!(a.destructive_hint, Some(false));
        assert_eq!(a.read_only_hint, Some(false));
        assert_eq!(a.idempotent_hint, Some(false));
        assert_eq!(a.open_world_hint, Some(true));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn list_tools_advertises_output_schema_for_exactly_the_structured_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());
        let result = handler
            .handle_list_tools_request(None, stub_runtime())
            .await
            .expect("list_tools must succeed");

        for tool in &result.tools {
            let expected =
                super::super::output_schema::STRUCTURED_TOOLS.contains(&tool.name.as_str());
            assert_eq!(
                tool.output_schema.is_some(),
                expected,
                "output schema advertisement mismatch for '{}'",
                tool.name
            );
            assert!(
                tool.annotations.is_some(),
                "tool '{}' must carry annotations",
                tool.name
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_run_experiment_persists_journal_and_ingests() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_valid_experiment(tmp.path());
        let handler = open_handler(tmp.path());
        let store_path = tmp.path().join("analytics.duckdb");

        let params = call_params(
            "tumult_run_experiment",
            serde_json::json!({
                "experiment_path": "test.toon",
                "store_path": store_path.to_str().unwrap(),
            }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("run must produce a result");
        assert!(result.is_error.is_none(), "{}", result_text(&result));

        // Journal persisted under the CLI-default name in the workspace root.
        let journal_file = tmp.path().join("journal.toon");
        assert!(journal_file.exists(), "journal.toon must be written");
        let journal = tumult_core::journal::read_journal(&journal_file).unwrap();
        assert_eq!(journal.experiment_title, "MCP test experiment");

        // Auto-ingested into the analytics store.
        let store = tumult_analytics::AnalyticsStore::open(&store_path).unwrap();
        assert_eq!(store.stats().unwrap().experiment_count, 1);

        // structuredContent present, conforming, and mirrored in the text.
        let structured = result
            .structured_content
            .as_ref()
            .expect("run must set structuredContent");
        assert_conforms("tumult_run_experiment", structured);
        assert_eq!(structured["ingestion"], "ingested");
        assert_eq!(
            structured["journal"]["experiment_title"],
            "MCP test experiment"
        );
        let text_json: serde_json::Value =
            serde_json::from_str(&result_text(&result)).expect("json text content");
        assert_eq!(text_json["journal_path"], structured["journal_path"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_run_experiment_honors_journal_path_and_no_ingest() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_valid_experiment(tmp.path());
        let handler = open_handler(tmp.path());
        let store_path = tmp.path().join("analytics.duckdb");

        let params = call_params(
            "tumult_run_experiment",
            serde_json::json!({
                "experiment_path": "test.toon",
                "journal_path": "custom-run.toon",
                "no_ingest": true,
                "store_path": store_path.to_str().unwrap(),
                "format": "toon",
            }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("run must produce a result");
        assert!(result.is_error.is_none(), "{}", result_text(&result));

        assert!(tmp.path().join("custom-run.toon").exists());
        assert!(!tmp.path().join("journal.toon").exists());
        assert!(!store_path.exists(), "no_ingest must not create the store");
        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["ingestion"], "skipped");
        // TOON text format keeps the 2.0.0-era journal payload shape.
        assert!(result_text(&result).contains("MCP test experiment"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_run_experiment_rejects_unknown_rollback_strategy() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_valid_experiment(tmp.path());
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_run_experiment",
            serde_json::json!({
                "experiment_path": "test.toon",
                "rollback_strategy": "sometimes",
                "no_ingest": true,
            }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("tool-level failure must still yield a CallToolResult");
        assert_eq!(result.is_error, Some(true));
        let text = result_text(&result);
        assert!(
            text.contains("sometimes"),
            "must name the bad value: {text}"
        );
        assert!(
            text.contains("on-deviation") && text.contains("always") && text.contains("never"),
            "must list valid values: {text}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_read_journal_structured_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_run_journal(tmp.path());
        let handler = open_handler(tmp.path());

        // Default: full journal as JSON.
        let params = call_params(
            "tumult_read_journal",
            serde_json::json!({ "journal_path": "journal.toon" }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("read must produce a result");
        assert!(result.is_error.is_none(), "{}", result_text(&result));
        let structured = result
            .structured_content
            .as_ref()
            .expect("read must set structuredContent");
        assert_conforms("tumult_read_journal", structured);
        assert_eq!(
            structured["journal"]["experiment_title"],
            "MCP test experiment"
        );
        // Text is the same object rendered as JSON.
        let text_json: serde_json::Value =
            serde_json::from_str(&result_text(&result)).expect("json text content");
        assert_eq!(
            text_json,
            serde_json::Value::Object(structured.clone()),
            "text content must mirror structuredContent"
        );

        // summary=true drops the full journal but keeps the summary.
        let params = call_params(
            "tumult_read_journal",
            serde_json::json!({ "journal_path": "journal.toon", "summary": true }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        let structured = result.structured_content.as_ref().unwrap();
        assert_conforms("tumult_read_journal", structured);
        assert!(structured.contains_key("summary"));
        assert!(!structured.contains_key("journal"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn structured_content_conforms_to_advertised_schema_for_all_structured_tools() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_valid_experiment(tmp.path());
        let handler = open_handler(tmp.path());
        let store_path = tmp.path().join("analytics.duckdb");
        let missing_store = tmp.path().join("missing.duckdb");
        drop(tumult_analytics::AnalyticsStore::open(&store_path).unwrap());

        // Ordered so the run writes journal.toon before read_journal uses it.
        let calls: Vec<(&str, serde_json::Value)> = vec![
            (
                "tumult_run_experiment",
                serde_json::json!({
                    "experiment_path": "test.toon",
                    "store_path": store_path.to_str().unwrap(),
                }),
            ),
            (
                "tumult_read_journal",
                serde_json::json!({ "journal_path": "journal.toon" }),
            ),
            (
                "tumult_report",
                serde_json::json!({ "journal_path": "journal.toon", "format": "junit" }),
            ),
            (
                "tumult_compliance",
                serde_json::json!({ "journals_path": "journal.toon", "framework": "dora" }),
            ),
            (
                "tumult_trend",
                serde_json::json!({ "journals_path": "journal.toon", "metric": "duration_ms" }),
            ),
            (
                "tumult_gameday_create",
                serde_json::json!({ "name": "conformance-gd", "experiments": ["test.toon"] }),
            ),
            ("tumult_agents", serde_json::json!({})),
            (
                "tumult_recommend",
                serde_json::json!({ "store_path": missing_store.to_str().unwrap() }),
            ),
            (
                "tumult_store_stats",
                serde_json::json!({ "store_path": store_path.to_str().unwrap() }),
            ),
            (
                "tumult_coverage",
                serde_json::json!({ "store_path": missing_store.to_str().unwrap() }),
            ),
            ("tumult_agentic_list_scenarios", serde_json::json!({})),
            ("tumult_agentic_smoke", serde_json::json!({})),
            ("tumult_agentic_run_experiment", serde_json::json!({})),
            ("tumult_list_journals", serde_json::json!({ "path": "." })),
            ("tumult_list_experiments", serde_json::json!({})),
            ("tumult_gameday_list", serde_json::json!({})),
        ];

        // This test must exercise every tool that advertises an output schema.
        let mut covered: Vec<&str> = calls.iter().map(|(name, _)| *name).collect();
        covered.sort_unstable();
        let mut expected = super::super::output_schema::STRUCTURED_TOOLS.to_vec();
        expected.sort_unstable();
        assert_eq!(covered, expected, "every structured tool must be covered");

        for (name, args) in calls {
            let params = call_params(name, args, None);
            let result = handler
                .handle_call_tool_request(params, stub_runtime())
                .await
                .unwrap_or_else(|e| panic!("'{name}' must succeed: {e}"));
            assert!(
                result.is_error.is_none(),
                "'{name}' failed: {}",
                result_text(&result)
            );
            let structured = result
                .structured_content
                .as_ref()
                .unwrap_or_else(|| panic!("'{name}' must set structuredContent"));
            assert_conforms(name, structured);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_report_writes_output_file_and_returns_path() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_run_journal(tmp.path());
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_report",
            serde_json::json!({
                "journal_path": "journal.toon",
                "format": "junit",
                "output_path": "report.xml",
            }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("report must produce a result");
        assert!(result.is_error.is_none(), "{}", result_text(&result));

        let out = tmp.path().join("report.xml");
        assert!(out.exists(), "report file must be written");
        let xml = std::fs::read_to_string(&out).unwrap();
        assert!(xml.contains("<testsuite name=\"MCP test experiment\""));

        let structured = result.structured_content.as_ref().unwrap();
        assert_conforms("tumult_report", structured);
        assert!(!structured.contains_key("content"));
        assert!(result_text(&result).contains("Report generated"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_report_inline_json_and_rejects_html() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_run_journal(tmp.path());
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_report",
            serde_json::json!({ "journal_path": "journal.toon" }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert!(result.is_error.is_none(), "{}", result_text(&result));
        let parsed: serde_json::Value =
            serde_json::from_str(&result_text(&result)).expect("inline json report");
        assert_eq!(parsed["experiment_title"], "MCP test experiment");

        // html/pdf are CLI-only; the tool must reject them explicitly.
        let params = call_params(
            "tumult_report",
            serde_json::json!({ "journal_path": "journal.toon", "format": "html" }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let text = result_text(&result);
        assert!(
            text.contains("json") && text.contains("junit"),
            "must list valid values: {text}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_compliance_rejects_unknown_framework() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_run_journal(tmp.path());
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_compliance",
            serde_json::json!({ "journals_path": "journal.toon", "framework": "hipaa" }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let text = result_text(&result);
        assert!(text.contains("hipaa"), "must name the bad value: {text}");
        assert!(
            text.contains("dora") && text.contains("pci-dss") && text.contains("basel-iii"),
            "must list valid values: {text}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_trend_rejects_unknown_metric() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_run_journal(tmp.path());
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_trend",
            serde_json::json!({ "journals_path": "journal.toon", "metric": "latency" }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let text = result_text(&result);
        assert!(
            text.contains("resilience_score") && text.contains("method_step_count"),
            "must list valid metrics: {text}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_gameday_create_round_trip_writes_parseable_campaign() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_valid_experiment(tmp.path());
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_gameday_create",
            serde_json::json!({
                "name": "q3-drill",
                "experiments": ["test.toon"],
                "load_tool": "k6",
                "load_script": "load.js",
                "load_vus": 5,
                "framework": "dora",
            }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("gameday_create must produce a result");
        assert!(result.is_error.is_none(), "{}", result_text(&result));

        let created = tmp.path().join("q3-drill.gameday.toon");
        assert!(created.exists(), "gameday file must be written");
        let content = std::fs::read_to_string(&created).unwrap();
        let gameday: tumult_core::types::GameDay =
            toon_format::decode_default(&content).expect("created file must parse as GameDay");
        assert_eq!(gameday.title, "q3-drill");
        assert_eq!(gameday.experiments.len(), 1);
        assert!(gameday.load.is_some());
        assert!(content.contains("frameworks[1]: DORA"));

        let structured = result.structured_content.as_ref().unwrap();
        assert_conforms("tumult_gameday_create", structured);
        assert_eq!(structured["experiments"], 1);

        // Second create with the same name must fail (no overwrite).
        let params = call_params(
            "tumult_gameday_create",
            serde_json::json!({ "name": "q3-drill", "experiments": ["test.toon"] }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(result_text(&result).contains("already exists"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_gameday_create_rejects_name_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_gameday_create",
            serde_json::json!({ "name": "../escape", "experiments": ["test.toon"] }),
            None,
        );
        assert!(
            handler
                .handle_call_tool_request(params, stub_runtime())
                .await
                .is_err(),
            "gameday name traversal must be rejected at the dispatch boundary"
        );
        assert!(
            !tmp.path()
                .parent()
                .unwrap()
                .join("escape.gameday.toon")
                .exists(),
            "no file must be written outside the workspace"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_agents_round_trip_lists_builtin_adapters() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());

        let params = call_params("tumult_agents", serde_json::json!({}), None);
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("agents must produce a result");
        assert!(result.is_error.is_none(), "{}", result_text(&result));

        let structured = result.structured_content.as_ref().unwrap();
        assert_conforms("tumult_agents", structured);
        let names: Vec<&str> = structured["adapters"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"claude-code"), "adapters: {names:?}");
        assert!(names.contains(&"codex"), "adapters: {names:?}");
        assert!(result_text(&result).contains("ADAPTER"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_recommend_rejects_agent_params_without_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_recommend",
            serde_json::json!({ "agent_model": "opus" }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        assert!(
            result_text(&result).contains("require the agent parameter"),
            "got: {}",
            result_text(&result)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_recommend_rejects_unknown_adapter_listing_available() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_recommend",
            serde_json::json!({ "agent": "no-such-agent" }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let text = result_text(&result);
        assert!(
            text.contains("no-such-agent"),
            "must name the bad adapter: {text}"
        );
        assert!(
            text.contains("claude-code") && text.contains("codex"),
            "must list available adapters: {text}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_recommend_heuristic_over_real_store_matches_intelligence_shape() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_valid_experiment(tmp.path());
        let handler = open_handler(tmp.path());
        let store_path = tmp.path().join("analytics.duckdb");

        // Populate the store through a real run.
        let params = call_params(
            "tumult_run_experiment",
            serde_json::json!({
                "experiment_path": "test.toon",
                "store_path": store_path.to_str().unwrap(),
            }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert!(result.is_error.is_none(), "{}", result_text(&result));

        let params = call_params(
            "tumult_recommend",
            serde_json::json!({ "store_path": store_path.to_str().unwrap() }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert!(result.is_error.is_none(), "{}", result_text(&result));
        let structured = result.structured_content.as_ref().unwrap();
        assert_conforms("tumult_recommend", structured);
        // The tumult-intelligence pipeline is the source of truth now.
        assert_eq!(structured["source"], "heuristic-fallback");
        assert!(structured["heuristic_context"]
            .as_str()
            .unwrap()
            .contains("Coverage:"));
        assert!(result_text(&result).contains("AI-Powered Tumult Recommendations"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_list_journals_accepts_path_argument() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.toon"), "x").unwrap();
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_list_journals",
            serde_json::json!({ "path": "." }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("list_journals must accept the unified `path` parameter");
        assert!(result_text(&result).contains("a.toon"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_validate_round_trip_returns_summary() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_valid_experiment(tmp.path());
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_validate",
            serde_json::json!({ "experiment_path": "test.toon" }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("valid experiment must produce a result");

        let text = result_text(&result);
        assert!(
            text.contains("Valid: 'MCP test experiment'"),
            "unexpected payload: {text}"
        );
        assert!(
            text.contains("1 method steps"),
            "unexpected payload: {text}"
        );
        assert!(
            !text.starts_with("Error:"),
            "validation must not report an error: {text}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_list_journals_round_trip_filters_toon_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.toon"), "x").unwrap();
        std::fs::write(tmp.path().join("b.toon"), "x").unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "x").unwrap();
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_list_journals",
            serde_json::json!({ "directory": "." }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("list_journals must produce a result");

        let text = result_text(&result);
        assert!(text.contains("a.toon"), "a.toon missing from: {text}");
        assert!(text.contains("b.toon"), "b.toon missing from: {text}");
        assert!(
            !text.contains("notes.txt"),
            "non-.toon file must be filtered out: {text}"
        );
    }

    // ── resource_link content items ───────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_run_experiment_attaches_journal_resource_link() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_valid_experiment(tmp.path());
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_run_experiment",
            serde_json::json!({ "experiment_path": "test.toon", "no_ingest": true }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert!(result.is_error.is_none(), "{}", result_text(&result));

        let links = result_links(&result);
        assert_eq!(links.len(), 1, "run must link the written journal");
        assert_eq!(links[0].uri, "tumult://journal/journal.toon");
        assert_eq!(links[0].mime_type.as_deref(), Some("application/json"));
        // Text content is still the first block (backward compat).
        assert!(matches!(result.content[0], ContentBlock::TextContent(_)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_gameday_create_attaches_gameday_resource_link() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_valid_experiment(tmp.path());
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_gameday_create",
            serde_json::json!({ "name": "linked", "experiments": ["test.toon"] }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert!(result.is_error.is_none(), "{}", result_text(&result));

        let links = result_links(&result);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].uri, "tumult://gameday/linked.gameday.toon");
        assert_eq!(links[0].mime_type.as_deref(), Some("application/toon"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_report_attaches_link_only_when_output_path_given() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_run_journal(tmp.path());
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_report",
            serde_json::json!({
                "journal_path": "journal.toon",
                "format": "junit",
                "output_path": "report.xml",
            }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        let links = result_links(&result);
        assert_eq!(links.len(), 1, "written report must be linked");
        assert!(links[0].uri.starts_with("file://"), "uri: {}", links[0].uri);
        assert!(
            links[0].uri.ends_with("/report.xml"),
            "uri: {}",
            links[0].uri
        );
        assert_eq!(links[0].mime_type.as_deref(), Some("application/xml"));

        // Inline report (no output_path): no link.
        let params = call_params(
            "tumult_report",
            serde_json::json!({ "journal_path": "journal.toon" }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert!(result_links(&result).is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_list_journals_attaches_links_capped_at_fifty() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..55 {
            std::fs::write(tmp.path().join(format!("j-{i:02}.toon")), "x").unwrap();
        }
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_list_journals",
            serde_json::json!({ "path": "." }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert!(result.is_error.is_none(), "{}", result_text(&result));

        let links = result_links(&result);
        assert_eq!(links.len(), 50, "links must be capped at the first 50");
        assert_eq!(links[0].uri, "tumult://journal/j-00.toon");
        // All 55 paths still appear in the text and structured items.
        let structured = result.structured_content.as_ref().unwrap();
        assert_eq!(structured["total"], 55);
        assert_eq!(structured["items"].as_array().unwrap().len(), 55);
    }

    // ── list tool pagination ──────────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_list_journals_honors_limit_offset_and_totals() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["a.toon", "b.toon", "c.toon"] {
            std::fs::write(tmp.path().join(name), "x").unwrap();
        }
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_list_journals",
            serde_json::json!({ "path": ".", "limit": 1, "offset": 1 }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        assert!(result.is_error.is_none(), "{}", result_text(&result));

        let structured = result.structured_content.as_ref().unwrap();
        assert_conforms("tumult_list_journals", structured);
        assert_eq!(structured["total"], 3);
        assert_eq!(structured["offset"], 1);
        assert_eq!(structured["limit"], 1);
        let items = structured["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert!(items[0].as_str().unwrap().ends_with("b.toon"));
        let text = result_text(&result);
        assert!(
            text.contains("b.toon") && !text.contains("a.toon"),
            "{text}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_list_tools_reject_limit_over_max() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());

        for (name, mut args) in [
            ("tumult_list_journals", serde_json::json!({ "path": "." })),
            ("tumult_list_experiments", serde_json::json!({})),
            ("tumult_gameday_list", serde_json::json!({})),
        ] {
            args["limit"] = serde_json::json!(1001);
            let err = handler
                .handle_call_tool_request(call_params(name, args, None), stub_runtime())
                .await
                .expect_err("limit over 1000 must be rejected");
            assert!(err.to_string().contains("1000"), "{name}: {err}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_list_experiments_and_gamedays_paginate_with_totals() {
        let tmp = tempfile::tempdir().unwrap();
        for i in 0..4 {
            std::fs::write(
                tmp.path().join(format!("exp-{i}.toon")),
                format!("title: Experiment {i}\n"),
            )
            .unwrap();
            std::fs::write(
                tmp.path().join(format!("gd-{i}.gameday.toon")),
                format!("title: GD {i}\n"),
            )
            .unwrap();
        }
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_list_experiments",
            serde_json::json!({ "limit": 2, "offset": 2 }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        let structured = result.structured_content.as_ref().unwrap();
        assert_conforms("tumult_list_experiments", structured);
        // The 4 .gameday.toon files also carry titles, so they are counted
        // as experiments by the recursive .toon discovery (existing tool
        // behavior); totals must reflect everything found.
        assert_eq!(structured["total"], 8);
        assert_eq!(structured["items"].as_array().unwrap().len(), 2);

        let params = call_params(
            "tumult_gameday_list",
            serde_json::json!({ "limit": 3, "offset": 2 }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .unwrap();
        let structured = result.structured_content.as_ref().unwrap();
        assert_conforms("tumult_gameday_list", structured);
        assert_eq!(structured["total"], 4);
        assert_eq!(structured["offset"], 2);
        let items = structured["items"].as_array().unwrap();
        assert_eq!(items.len(), 2, "only two entries remain after offset 2");
        assert_eq!(items[0]["title"], "GD 2");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_tool_failure_is_reported_as_error_text() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("bad.toon"), "not a valid experiment {{{").unwrap();
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_validate",
            serde_json::json!({ "experiment_path": "bad.toon" }),
            None,
        );
        // Tool-level failures are embedded in the result payload (per MCP spec),
        // not surfaced as protocol errors.
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("tool failure must still yield a CallToolResult");

        let text = result_text(&result);
        assert!(
            text.starts_with("Error:"),
            "tool failure must be reported in the payload: {text}"
        );
        assert_eq!(
            result.is_error,
            Some(true),
            "tool failure must set isError so clients can detect it"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_create_experiment_round_trip_writes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_create_experiment",
            serde_json::json!({ "output_path": "new-experiment.toon" }),
            None,
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("create_experiment must produce a result");

        let text = result_text(&result);
        assert!(
            text.starts_with("Created"),
            "creation must be reported in the payload: {text}"
        );
        assert!(
            result.is_error.is_none(),
            "successful creation must not set isError"
        );
        let created = tmp.path().join("new-experiment.toon");
        assert!(created.exists(), "output file must exist in the workspace");
        let content = std::fs::read_to_string(&created).unwrap();
        assert!(content.contains("title:"), "template must be written");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_create_experiment_rejects_output_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_create_experiment",
            serde_json::json!({ "output_path": "../escape.toon" }),
            None,
        );
        assert!(
            handler
                .handle_call_tool_request(params, stub_runtime())
                .await
                .is_err(),
            "output path traversal must be rejected at the dispatch boundary"
        );
        assert!(
            !tmp.path().parent().unwrap().join("escape.toon").exists(),
            "no file must be written outside the workspace"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_unknown_name_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());

        let params = call_params("tumult_no_such_tool", serde_json::json!({}), None);
        let err = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect_err("unknown tool must be a protocol-level error");
        assert!(
            err.to_string().contains("tumult_no_such_tool"),
            "error must name the unknown tool: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_missing_required_argument_returns_error_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());

        // `tumult_validate` requires `experiment_path`; omit it entirely.
        let params = call_params("tumult_validate", serde_json::json!({}), None);
        let err = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect_err("missing required argument must be an error");
        assert!(
            err.to_string().contains("experiment_path"),
            "error must mention the missing field: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_wrong_argument_type_returns_error_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());

        // Wrong type (number instead of string) and absent arguments object
        // must both fail gracefully.
        let params = call_params(
            "tumult_validate",
            serde_json::json!({ "experiment_path": 42 }),
            None,
        );
        assert!(handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .is_err());

        let params = call_params("tumult_validate", serde_json::Value::Null, None);
        assert!(handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_path_traversal_rejected_through_dispatch() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());

        let params = call_params(
            "tumult_validate",
            serde_json::json!({ "experiment_path": "../../etc/passwd" }),
            None,
        );
        assert!(
            handler
                .handle_call_tool_request(params, stub_runtime())
                .await
                .is_err(),
            "path traversal must be rejected at the dispatch boundary"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_rejects_wrong_bearer_token() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_valid_experiment(tmp.path());
        let handler = TumultHandler::with_auth(
            tmp.path().to_path_buf(),
            McpAuth {
                token: Some("dispatch-secret".into()),
            },
        );

        let params = call_params(
            "tumult_validate",
            serde_json::json!({ "experiment_path": "test.toon" }),
            Some("Bearer wrong-token"),
        );
        let err = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect_err("wrong bearer token must be rejected");
        assert!(
            err.to_string().contains("Unauthorized"),
            "error must indicate authorization failure: {err}"
        );
        assert!(
            !err.to_string().contains("Unknown tool"),
            "auth failure must not masquerade as an unknown tool: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_rejects_missing_token_when_auth_configured() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = TumultHandler::with_auth(
            tmp.path().to_path_buf(),
            McpAuth {
                token: Some("dispatch-secret".into()),
            },
        );

        // No `_meta.authorization` supplied at all.
        let params = call_params("tumult_discover", serde_json::json!({}), None);
        let err = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect_err("missing token must be rejected when auth is configured");
        assert!(err.to_string().contains("Unauthorized"), "got: {err}");
        assert!(
            !err.to_string().contains("Unknown tool"),
            "auth failure must not masquerade as an unknown tool: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_accepts_correct_bearer_token() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_valid_experiment(tmp.path());
        let handler = TumultHandler::with_auth(
            tmp.path().to_path_buf(),
            McpAuth {
                token: Some("dispatch-secret".into()),
            },
        );

        let params = call_params(
            "tumult_validate",
            serde_json::json!({ "experiment_path": "test.toon" }),
            Some("Bearer dispatch-secret"),
        );
        let result = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect("correct bearer token must be accepted");
        assert!(
            result_text(&result).contains("Valid: 'MCP test experiment'"),
            "authorized call must reach the tool"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_closed_semaphore_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let handler = open_handler(tmp.path());
        handler.semaphore.close();

        let params = call_params("tumult_discover", serde_json::json!({}), None);
        let err = handler
            .handle_call_tool_request(params, stub_runtime())
            .await
            .expect_err("closed semaphore must fail the call");
        assert!(err.to_string().contains("semaphore closed"), "got: {err}");
        assert!(
            !err.to_string().contains("Unknown tool"),
            "semaphore failure must not masquerade as an unknown tool: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_releases_semaphore_permit_after_completion() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_valid_experiment(tmp.path());
        let handler = open_handler(tmp.path());

        for _ in 0..3 {
            let params = call_params(
                "tumult_validate",
                serde_json::json!({ "experiment_path": "test.toon" }),
                None,
            );
            handler
                .handle_call_tool_request(params, stub_runtime())
                .await
                .expect("call must succeed");
        }
        assert_eq!(
            handler.semaphore.available_permits(),
            MAX_CONCURRENT_TOOL_CALLS,
            "permits must be released after each call"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn call_tool_waits_while_semaphore_saturated_and_resumes_on_release() {
        let tmp = tempfile::tempdir().unwrap();
        crate::tools::test_support::write_valid_experiment(tmp.path());
        let handler = Arc::new(open_handler(tmp.path()));

        // Saturate the semaphore.
        let permits = handler
            .semaphore
            .acquire_many(u32::try_from(MAX_CONCURRENT_TOOL_CALLS).unwrap())
            .await
            .unwrap();

        let call_handler = Arc::clone(&handler);
        let task = tokio::spawn(async move {
            let params = call_params(
                "tumult_validate",
                serde_json::json!({ "experiment_path": "test.toon" }),
                None,
            );
            // CallToolError is not Send; map both arms to Send types before
            // crossing the task boundary.
            call_handler
                .handle_call_tool_request(params, stub_runtime())
                .await
                .map(|result| result_text(&result))
                .map_err(|e| e.to_string())
        });

        // Give the spawned call ample opportunity to run: it must stay parked
        // on the semaphore while all permits are held.
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert!(
            !task.is_finished(),
            "call must not proceed while the semaphore is saturated"
        );

        // Releasing the permits must let the call complete.
        drop(permits);
        let text = task
            .await
            .unwrap()
            .expect("call must succeed after release");
        assert!(text.contains("Valid: 'MCP test experiment'"));
    }
}
