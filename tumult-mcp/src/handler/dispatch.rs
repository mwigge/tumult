//! `ServerHandler` implementation — lists tools and dispatches tool calls.

use std::sync::Arc;

use async_trait::async_trait;
use rust_mcp_sdk::{
    mcp_server::ServerHandler,
    schema::{
        CallToolError, CallToolRequestParams, CallToolResult, ListToolsResult,
        PaginatedRequestParams, RpcError,
    },
    McpServer,
};

use crate::tools;

use super::schema::{
    AgenticListScenariosTool, AgenticRunExperimentTool, AgenticSmokeTool, AnalyzeStoreTool,
    AnalyzeTool, CoverageTool, CreateExperimentTool, DiscoverTool, GameDayAnalyzeTool,
    GameDayListTool, GameDayRunTool, ListExperimentsTool, ListJournalsTool, QueryTracesTool,
    ReadJournalTool, RecommendTool, RunExperimentTool, StoreStatsTool, ValidateTool,
};
use super::TumultHandler;

#[async_trait]
impl ServerHandler for TumultHandler {
    async fn handle_list_tools_request(
        &self,
        _params: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ListToolsResult, RpcError> {
        Ok(ListToolsResult {
            tools: vec![
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
                GameDayRunTool::tool(),
                GameDayAnalyzeTool::tool(),
                GameDayListTool::tool(),
                RecommendTool::tool(),
                CoverageTool::tool(),
                AgenticListScenariosTool::tool(),
                AgenticSmokeTool::tool(),
                AgenticRunExperimentTool::tool(),
            ],
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
            .map_err(|_| CallToolError::unknown_tool("semaphore closed".to_string()))?;

        // Enforce bearer token authentication if configured.
        // Clients pass the Authorization value via `_meta.authorization` since
        // stdio transport has no HTTP header context at the handler level.
        let authorization = Self::extract_authorization(&params);
        if let Err(e) = self.auth.check(authorization.as_deref()) {
            return Err(CallToolError::unknown_tool(format!("Unauthorized: {e}")));
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
                tokio::task::block_in_place(|| {
                    tools::run_experiment(&path, &args.rollback_strategy, Some(mcp_context))
                })
            }
            "tumult_validate" => {
                let args: ValidateTool = parse_args(&params)?;
                let path = self.resolve_path(&args.experiment_path)?;
                tokio::task::block_in_place(|| tools::validate_experiment(&path))
            }
            "tumult_analyze" => {
                let args: AnalyzeTool = parse_args(&params)?;
                let path = self.resolve_path(&args.journals_path)?;
                tokio::task::block_in_place(|| tools::analyze(&path, &args.query))
            }
            "tumult_read_journal" => {
                let args: ReadJournalTool = parse_args(&params)?;
                let path = self.resolve_path(&args.journal_path)?;
                tokio::task::block_in_place(|| tools::read_journal(&path))
            }
            "tumult_list_journals" => {
                let args: ListJournalsTool = parse_args(&params)?;
                let path = self.resolve_path(&args.directory)?;
                tokio::task::block_in_place(|| tools::list_journals(&path).map(|v| v.join("\n")))
            }
            "tumult_discover" => tokio::task::block_in_place(|| Ok(tools::discover_plugins())),
            "tumult_create_experiment" => {
                let args: CreateExperimentTool = parse_args(&params)?;
                let path = self.resolve_path(&args.output_path)?;
                tokio::task::block_in_place(|| {
                    tools::create_experiment(&path, args.plugin.as_deref())
                })
            }
            "tumult_query_traces" => {
                let args: QueryTracesTool = parse_args(&params)?;
                let path = self.resolve_path(&args.journal_path)?;
                tokio::task::block_in_place(|| tools::query_traces(&path))
            }
            "tumult_store_stats" => {
                let args: StoreStatsTool = parse_args(&params)?;
                tokio::task::block_in_place(|| tools::store_stats(&args.store_path))
            }
            "tumult_analyze_store" => {
                let args: AnalyzeStoreTool = parse_args(&params)?;
                tokio::task::block_in_place(|| {
                    tools::analyze_persistent(&args.store_path, &args.query)
                })
            }
            "tumult_list_experiments" => {
                let args: ListExperimentsTool = parse_args(&params)?;
                let search_root = if let Some(ref p) = args.path {
                    self.resolve_path(p)?
                } else {
                    self.workspace_root_str()?
                };
                tokio::task::block_in_place(|| tools::list_experiments(&search_root))
            }
            "tumult_gameday_run" => {
                let args: GameDayRunTool = parse_args(&params)?;
                let path = self.resolve_path(&args.gameday_path)?;
                tokio::task::block_in_place(|| tools::gameday_run(&path))
            }
            "tumult_gameday_analyze" => {
                let args: GameDayAnalyzeTool = parse_args(&params)?;
                let path = self.resolve_path(&args.gameday_path)?;
                tokio::task::block_in_place(|| tools::gameday_analyze(&path))
            }
            "tumult_gameday_list" => {
                let args: GameDayListTool = parse_args(&params)?;
                let search_root = if let Some(ref p) = args.path {
                    self.resolve_path(p)?
                } else {
                    self.workspace_root_str()?
                };
                tokio::task::block_in_place(|| tools::gameday_list(&search_root))
            }
            "tumult_recommend" => {
                let args: RecommendTool = parse_args(&params)?;
                tokio::task::block_in_place(|| {
                    tools::recommend(
                        &args.store_path,
                        args.goal.as_deref(),
                        args.model.as_deref(),
                        args.include_draft,
                        &args.format,
                    )
                })
            }
            "tumult_coverage" => {
                let args: CoverageTool = parse_args(&params)?;
                tokio::task::block_in_place(|| tools::coverage(&args.store_path))
            }
            "tumult_agentic_list_scenarios" => {
                let _args: AgenticListScenariosTool = parse_args(&params)?;
                tokio::task::block_in_place(tools::agentic_list_scenarios)
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
                result
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
                result
            }
            _ => return Err(CallToolError::unknown_tool(params.name)),
        };

        match result {
            Ok(content) => {
                crate::telemetry::event_tool_completed(&params.name, true);
                Ok(CallToolResult::text_content(vec![content.into()]))
            }
            Err(e) => {
                crate::telemetry::event_tool_error(&params.name, &e.to_string());
                Ok(CallToolResult::text_content(vec![
                    format!("Error: {e}").into()
                ]))
            }
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
            GameDayRunTool::tool(),
            GameDayAnalyzeTool::tool(),
            GameDayListTool::tool(),
            RecommendTool::tool(),
            CoverageTool::tool(),
            AgenticListScenariosTool::tool(),
            AgenticSmokeTool::tool(),
            AgenticRunExperimentTool::tool(),
        ];
        assert_eq!(tools.len(), 19);
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
}
