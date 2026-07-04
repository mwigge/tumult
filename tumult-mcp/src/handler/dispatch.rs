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
                let path = self.resolve_output_path(&args.output_path)?;
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
                // Per MCP spec, tool-level failures are reported inside the
                // result with `isError: true`, not as protocol errors.
                let mut result = CallToolResult::text_content(vec![format!("Error: {e}").into()]);
                result.is_error = Some(true);
                Ok(result)
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

    // ── ServerHandler round trips (dispatch) ─────────────────────
    //
    // These tests invoke `handle_list_tools_request` / `handle_call_tool_request`
    // directly against a stub `McpServer` runtime — no transport, no network.

    use std::time::Duration;

    use rust_mcp_sdk::auth::AuthInfo;
    use rust_mcp_sdk::error::SdkResult;
    use rust_mcp_sdk::schema::{
        CallToolMeta, ClientMessage, ContentBlock, Implementation, InitializeRequestParams,
        InitializeResult, MessageFromServer, ProtocolVersion, RequestId, ServerCapabilities,
        ServerMessage,
    };
    use rust_mcp_sdk::task_store::{ClientTaskStore, ServerTaskStore};
    use rust_mcp_sdk::SessionId;

    /// Minimal `McpServer` runtime stub. Dispatch never touches the runtime,
    /// so every method is inert; `server_info` returns a static placeholder.
    struct StubMcpServer {
        details: InitializeResult,
        auth_info: tokio::sync::RwLock<Option<AuthInfo>>,
    }

    impl StubMcpServer {
        fn new() -> Self {
            Self {
                details: InitializeResult {
                    capabilities: ServerCapabilities::default(),
                    instructions: None,
                    meta: None,
                    protocol_version: ProtocolVersion::V2025_11_25.into(),
                    server_info: Implementation {
                        name: "stub".into(),
                        version: "0.0.0".into(),
                        title: None,
                        description: None,
                        icons: vec![],
                        website_url: None,
                    },
                },
                auth_info: tokio::sync::RwLock::new(None),
            }
        }
    }

    #[async_trait]
    impl McpServer for StubMcpServer {
        async fn start(self: Arc<Self>) -> SdkResult<()> {
            Ok(())
        }

        async fn set_client_details(
            &self,
            _client_details: InitializeRequestParams,
        ) -> SdkResult<()> {
            Ok(())
        }

        fn server_info(&self) -> &InitializeResult {
            &self.details
        }

        fn client_info(&self) -> Option<InitializeRequestParams> {
            None
        }

        async fn auth_info(&self) -> tokio::sync::RwLockReadGuard<'_, Option<AuthInfo>> {
            self.auth_info.read().await
        }

        async fn auth_info_cloned(&self) -> Option<AuthInfo> {
            self.auth_info.read().await.clone()
        }

        async fn update_auth_info(&self, auth_info: Option<AuthInfo>) {
            *self.auth_info.write().await = auth_info;
        }

        async fn wait_for_initialization(&self) {}

        fn task_store(&self) -> Option<Arc<ServerTaskStore>> {
            None
        }

        fn client_task_store(&self) -> Option<Arc<ClientTaskStore>> {
            None
        }

        async fn stderr_message(&self, _message: String) -> SdkResult<()> {
            Ok(())
        }

        fn session_id(&self) -> Option<SessionId> {
            None
        }

        async fn send(
            &self,
            _message: MessageFromServer,
            _request_id: Option<RequestId>,
            _request_timeout: Option<Duration>,
        ) -> SdkResult<Option<ClientMessage>> {
            Ok(None)
        }

        async fn send_batch(
            &self,
            _messages: Vec<ServerMessage>,
            _request_timeout: Option<Duration>,
        ) -> SdkResult<Option<Vec<ClientMessage>>> {
            Ok(None)
        }
    }

    fn stub_runtime() -> Arc<dyn McpServer> {
        Arc::new(StubMcpServer::new())
    }

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

    /// Concatenate all text content blocks of a `CallToolResult`.
    fn result_text(result: &CallToolResult) -> String {
        result
            .content
            .iter()
            .map(|block| match block {
                ContentBlock::TextContent(text) => text.text.clone(),
                _ => panic!("expected text content block"),
            })
            .collect::<Vec<_>>()
            .join("\n")
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
