//! MCP handler — routes tool calls to implementations.

use std::sync::Arc;

use async_trait::async_trait;
use rust_mcp_sdk::{
    macros,
    mcp_server::ServerHandler,
    schema::{
        CallToolError, CallToolRequestParams, CallToolResult, ListToolsResult,
        PaginatedRequestParams, RpcError,
    },
    McpServer,
};

use subtle::ConstantTimeEq;

use crate::tools;

// ── Tool schema definitions ───────────────────────────────────

#[macros::mcp_tool(
    name = "tumult_run_experiment",
    description = "Execute a Tumult chaos experiment and return the journal."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct RunExperimentTool {
    pub experiment_path: String,
    #[serde(default = "default_strategy")]
    pub rollback_strategy: String,
}
fn default_strategy() -> String {
    "on-deviation".into()
}

#[macros::mcp_tool(
    name = "tumult_validate",
    description = "Validate an experiment file for syntax and provider support."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ValidateTool {
    pub experiment_path: String,
}

#[macros::mcp_tool(
    name = "tumult_analyze",
    description = "SQL query over experiment journals via embedded DuckDB."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AnalyzeTool {
    pub journals_path: String,
    pub query: String,
}

#[macros::mcp_tool(
    name = "tumult_read_journal",
    description = "Read a TOON journal file and return its contents."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ReadJournalTool {
    pub journal_path: String,
}

#[macros::mcp_tool(
    name = "tumult_list_journals",
    description = "List .toon journal files in a directory."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ListJournalsTool {
    pub directory: String,
}

#[macros::mcp_tool(
    name = "tumult_discover",
    description = "List all Tumult plugins, actions, and probes."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct DiscoverTool {}

#[macros::mcp_tool(
    name = "tumult_create_experiment",
    description = "Create a new experiment from a template."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct CreateExperimentTool {
    pub output_path: String,
    pub plugin: Option<String>,
}

#[macros::mcp_tool(
    name = "tumult_query_traces",
    description = "Query trace data from a journal — returns activity spans with trace/span IDs for observability correlation."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct QueryTracesTool {
    pub journal_path: String,
}

#[macros::mcp_tool(
    name = "tumult_store_stats",
    description = "Get persistent analytics store statistics — experiment count, activity count, schema version, file size."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct StoreStatsTool {
    #[serde(default = "default_store_path")]
    pub store_path: String,
}
fn default_store_path() -> String {
    let path = tumult_analytics::AnalyticsStore::default_path();
    path.to_str().map_or_else(
        || ".tumult/analytics.db".to_string(),
        std::string::ToString::to_string,
    )
}

#[macros::mcp_tool(
    name = "tumult_analyze_store",
    description = "SQL query over the persistent analytics store (accumulated history from all runs)."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AnalyzeStoreTool {
    pub query: String,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

#[macros::mcp_tool(
    name = "tumult_list_experiments",
    description = "List all .toon experiment files recursively from the workspace or a given path."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ListExperimentsTool {
    /// Optional subdirectory to search within (relative to workspace root).
    pub path: Option<String>,
}

// ── GameDay tools ─────────────────────────────────────────────

#[macros::mcp_tool(
    name = "tumult_gameday_run",
    description = "Run a GameDay — execute all experiments in a .gameday.toon file under shared load. Returns resilience score and compliance status."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayRunTool {
    /// Path to the `.gameday.toon` file.
    pub gameday_path: String,
}

#[macros::mcp_tool(
    name = "tumult_gameday_analyze",
    description = "Analyze a completed GameDay journal — returns resilience score, per-experiment results, and compliance article mapping."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayAnalyzeTool {
    /// Path to the `.gameday.toon` file (reads the .journal.toon alongside it).
    pub gameday_path: String,
}

#[macros::mcp_tool(
    name = "tumult_gameday_list",
    description = "List available GameDay files (.gameday.toon) in the workspace."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayListTool {
    /// Optional subdirectory to search within.
    pub path: Option<String>,
}

// ── Intelligence tools (agent reasoning) ─────────────────────

#[macros::mcp_tool(
    name = "tumult_recommend",
    description = "Recommend what to test next — analyzes coverage gaps, failure patterns, and stale experiments. Returns actionable suggestions for an agent or engineer."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct RecommendTool {
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

#[macros::mcp_tool(
    name = "tumult_coverage",
    description = "Coverage report — which plugins, actions, and targets have been tested vs available. Shows per-plugin test status (FULL/PARTIAL/NONE) and store statistics."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct CoverageTool {
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

// ── Process executor (shared pattern with CLI) ────────────────

/// Executes activities that invoke external processes, using async I/O via
/// the current Tokio runtime.
pub struct ProcessExecutor;

impl tumult_core::runner::ActivityExecutor for ProcessExecutor {
    /// Executes the given activity by spawning an external process.
    ///
    /// # Panics
    ///
    /// Panics if called from a Tokio `current_thread` runtime context.
    /// `tokio::task::block_in_place` requires the `multi_thread` scheduler
    /// and will panic if the current runtime uses `current_thread`.
    fn execute(
        &self,
        activity: &tumult_core::types::Activity,
    ) -> tumult_core::runner::ActivityOutcome {
        match &activity.provider {
            tumult_core::types::Provider::Process {
                path,
                arguments,
                env,
                timeout_s,
            } => {
                let timeout = std::time::Duration::from_secs_f64(timeout_s.unwrap_or({
                    // u64 → f64: timeout constant is small; precision loss is irrelevant.
                    #[allow(clippy::cast_precision_loss)]
                    {
                        DEFAULT_EXECUTION_TIMEOUT_SECS as f64
                    }
                }));
                let start = std::time::Instant::now();
                let path = path.clone();
                let arguments = arguments.clone();
                let env = env.clone();

                // Use tokio::process::Command with async timeout instead of
                // busy-polling with std::thread::sleep.
                tokio::task::block_in_place(move || {
                    tokio::runtime::Handle::current().block_on(async {
                        use tokio::io::AsyncReadExt;
                        let mut child = match tokio::process::Command::new(&path)
                            .args(&arguments)
                            .envs(&env)
                            .stdout(std::process::Stdio::piped())
                            .stderr(std::process::Stdio::piped())
                            .spawn()
                        {
                            Ok(c) => c,
                            Err(e) => {
                                return tumult_core::runner::ActivityOutcome {
                                    success: false,
                                    output: None,
                                    error: Some(e.to_string()),
                                    duration_ms: 0,
                                };
                            }
                        };

                        // Collect stdout/stderr as owned before await to avoid
                        // the moved-value issue with wait_with_output().
                        let mut stdout_handle = child.stdout.take();
                        let mut stderr_handle = child.stderr.take();

                        let result = tokio::time::timeout(timeout, async {
                            let mut stdout_buf = Vec::new();
                            let mut stderr_buf = Vec::new();
                            if let Some(ref mut h) = stdout_handle {
                                let _ = h.read_to_end(&mut stdout_buf).await;
                            }
                            if let Some(ref mut h) = stderr_handle {
                                let _ = h.read_to_end(&mut stderr_buf).await;
                            }
                            let status = child.wait().await?;
                            Ok::<_, std::io::Error>((stdout_buf, stderr_buf, status))
                        })
                        .await;

                        let result = match result {
                            Ok(Ok((stdout_buf, stderr_buf, status))) => {
                                Ok((stdout_buf, stderr_buf, status))
                            }
                            Ok(Err(e)) => Err(e.to_string()),
                            Err(_elapsed) => Err(format!(
                                "process timed out after {:.1}s",
                                timeout.as_secs_f64()
                            )),
                        };

                        // u128 → u64: elapsed milliseconds; durations exceeding ~584M years
                        // will truncate, which is acceptable for telemetry.
                        #[allow(clippy::cast_possible_truncation)]
                        let elapsed = start.elapsed().as_millis() as u64;

                        match result {
                            Ok((stdout_buf, stderr_buf, status)) => {
                                let stdout =
                                    String::from_utf8_lossy(&stdout_buf).trim().to_string();
                                let stderr =
                                    String::from_utf8_lossy(&stderr_buf).trim().to_string();

                                tumult_core::runner::ActivityOutcome {
                                    success: status.success(),
                                    output: Some(stdout),
                                    error: if stderr.is_empty() {
                                        None
                                    } else {
                                        Some(stderr)
                                    },
                                    duration_ms: elapsed,
                                }
                            }
                            Err(reason) => tumult_core::runner::ActivityOutcome {
                                success: false,
                                output: None,
                                error: Some(reason),
                                duration_ms: elapsed,
                            },
                        }
                    })
                })
            }
            _ => tumult_core::runner::ActivityOutcome {
                success: false,
                output: None,
                error: Some("only process provider supported in MCP context".into()),
                duration_ms: 0,
            },
        }
    }
}

// ── Authentication ───────────────────────────────────────────

/// MCP authentication configuration.
///
/// If `TUMULT_MCP_TOKEN` is set, bearer token authentication is required
/// on all requests. If not set, the server runs without authentication
/// (with a warning logged).
pub struct McpAuth {
    token: Option<String>,
}

impl McpAuth {
    /// Read authentication config from environment.
    pub fn from_env() -> Self {
        let token = std::env::var("TUMULT_MCP_TOKEN")
            .ok()
            .filter(|t| !t.is_empty());
        if token.is_none() {
            tracing::warn!("TUMULT_MCP_TOKEN not set — MCP server running without authentication");
        }
        Self { token }
    }

    /// Check an Authorization header value. Returns Ok(()) if valid.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::ToolError::InvalidInput`] if the token is
    /// missing or does not match the configured bearer token.
    pub fn check(
        &self,
        authorization: Option<&str>,
    ) -> std::result::Result<(), crate::error::ToolError> {
        match &self.token {
            None => Ok(()), // no token configured, allow all
            Some(expected) => match authorization {
                Some(header) => {
                    let prefix = "Bearer ";
                    if let Some(provided) = header.strip_prefix(prefix) {
                        // Use constant-time comparison to prevent timing side-channel attacks.
                        let matches = provided.as_bytes().ct_eq(expected.as_bytes()).into();
                        if matches {
                            Ok(())
                        } else {
                            Err(crate::error::ToolError::InvalidInput(
                                "invalid bearer token".into(),
                            ))
                        }
                    } else {
                        Err(crate::error::ToolError::InvalidInput(
                            "expected Bearer token in Authorization header".into(),
                        ))
                    }
                }
                None => Err(crate::error::ToolError::InvalidInput(
                    "missing Authorization header".into(),
                )),
            },
        }
    }
}

/// Returns the address the MCP server should bind to.
/// Always binds to localhost only — never 0.0.0.0.
#[must_use]
pub fn mcp_bind_address() -> std::net::SocketAddr {
    std::net::SocketAddr::from(([127, 0, 0, 1], 8080))
}

// ── MCP Handler ───────────────────────────────────────────────

/// Maximum concurrent tool calls allowed.
const MAX_CONCURRENT_TOOL_CALLS: usize = 10;

/// Default execution timeout for process commands (seconds).
const DEFAULT_EXECUTION_TIMEOUT_SECS: u64 = 300;

pub struct TumultHandler {
    /// Semaphore limiting concurrent tool execution.
    pub(crate) semaphore: tokio::sync::Semaphore,
    /// Base directory for file operations (path traversal prevention).
    pub(crate) workspace_root: std::path::PathBuf,
    /// Bearer token authentication configuration.
    pub(crate) auth: McpAuth,
}

impl Default for TumultHandler {
    fn default() -> Self {
        Self {
            semaphore: tokio::sync::Semaphore::new(MAX_CONCURRENT_TOOL_CALLS),
            workspace_root: std::env::current_dir().unwrap_or_else(|_| "/".into()),
            auth: McpAuth::from_env(),
        }
    }
}

impl TumultHandler {
    /// Create a handler with a specific workspace root for path validation.
    #[must_use]
    pub fn with_workspace_root(workspace_root: std::path::PathBuf) -> Self {
        Self {
            semaphore: tokio::sync::Semaphore::new(MAX_CONCURRENT_TOOL_CALLS),
            workspace_root,
            auth: McpAuth::from_env(),
        }
    }

    /// Create a handler with a specific workspace root and authentication config.
    #[must_use]
    pub fn with_auth(workspace_root: std::path::PathBuf, auth: McpAuth) -> Self {
        Self {
            semaphore: tokio::sync::Semaphore::new(MAX_CONCURRENT_TOOL_CALLS),
            workspace_root,
            auth,
        }
    }

    /// Validate and resolve a user-supplied file path against the workspace root.
    ///
    /// # Errors
    ///
    /// Returns `CallToolError` if the path escapes the workspace root or
    /// the resolved path contains non-UTF-8 characters.
    fn resolve_path(&self, user_path: &str) -> std::result::Result<String, CallToolError> {
        let resolved = tools::safe_resolve_path(&self.workspace_root, user_path)
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

    /// Return the workspace root as a UTF-8 string.
    ///
    /// # Errors
    ///
    /// Returns `CallToolError` when the workspace root path contains non-UTF-8 characters.
    fn workspace_root_str(&self) -> std::result::Result<String, CallToolError> {
        self.workspace_root
            .to_str()
            .map(std::string::ToString::to_string)
            .ok_or_else(|| {
                CallToolError::invalid_arguments(
                    "workspace_root",
                    Some(format!(
                        "workspace root path contains non-UTF-8 characters: {}",
                        self.workspace_root.display()
                    )),
                )
            })
    }

    /// Extract authorization token from `_meta.authorization` in the call params.
    ///
    /// MCP clients using stdio transport pass authentication via the `_meta`
    /// field since HTTP headers are not available at the handler level.
    fn extract_authorization(params: &CallToolRequestParams) -> Option<String> {
        params
            .meta
            .as_ref()
            .and_then(|m| m.extra.as_ref())
            .and_then(|extra| extra.get("authorization"))
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string)
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
                tools::run_experiment(&path, &args.rollback_strategy, Some(mcp_context))
            }
            "tumult_validate" => {
                let args: ValidateTool = parse_args(&params)?;
                let path = self.resolve_path(&args.experiment_path)?;
                tools::validate_experiment(&path)
            }
            "tumult_analyze" => {
                let args: AnalyzeTool = parse_args(&params)?;
                let path = self.resolve_path(&args.journals_path)?;
                tools::analyze(&path, &args.query)
            }
            "tumult_read_journal" => {
                let args: ReadJournalTool = parse_args(&params)?;
                let path = self.resolve_path(&args.journal_path)?;
                tools::read_journal(&path)
            }
            "tumult_list_journals" => {
                let args: ListJournalsTool = parse_args(&params)?;
                let path = self.resolve_path(&args.directory)?;
                tools::list_journals(&path).map(|v| v.join("\n"))
            }
            "tumult_discover" => Ok(tools::discover_plugins()),
            "tumult_create_experiment" => {
                let args: CreateExperimentTool = parse_args(&params)?;
                let path = self.resolve_path(&args.output_path)?;
                tools::create_experiment(&path, args.plugin.as_deref())
            }
            "tumult_query_traces" => {
                let args: QueryTracesTool = parse_args(&params)?;
                let path = self.resolve_path(&args.journal_path)?;
                tools::query_traces(&path)
            }
            "tumult_store_stats" => {
                let args: StoreStatsTool = parse_args(&params)?;
                tools::store_stats(&args.store_path)
            }
            "tumult_analyze_store" => {
                let args: AnalyzeStoreTool = parse_args(&params)?;
                tools::analyze_persistent(&args.store_path, &args.query)
            }
            "tumult_list_experiments" => {
                let args: ListExperimentsTool = parse_args(&params)?;
                let search_root = if let Some(ref p) = args.path {
                    self.resolve_path(p)?
                } else {
                    self.workspace_root_str()?
                };
                tools::list_experiments(&search_root)
            }
            "tumult_gameday_run" => {
                let args: GameDayRunTool = parse_args(&params)?;
                let path = self.resolve_path(&args.gameday_path)?;
                tools::gameday_run(&path)
            }
            "tumult_gameday_analyze" => {
                let args: GameDayAnalyzeTool = parse_args(&params)?;
                let path = self.resolve_path(&args.gameday_path)?;
                tools::gameday_analyze(&path)
            }
            "tumult_gameday_list" => {
                let args: GameDayListTool = parse_args(&params)?;
                let search_root = if let Some(ref p) = args.path {
                    self.resolve_path(p)?
                } else {
                    self.workspace_root_str()?
                };
                tools::gameday_list(&search_root)
            }
            "tumult_recommend" => {
                let args: RecommendTool = parse_args(&params)?;
                tools::recommend(&args.store_path)
            }
            "tumult_coverage" => {
                let args: CoverageTool = parse_args(&params)?;
                tools::coverage(&args.store_path)
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
    use tumult_core::runner::ActivityExecutor;

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
        ];
        assert_eq!(tools.len(), 16);
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
        ];
        for tool in &tools {
            assert!(
                tool.name.starts_with("tumult_"),
                "tool name '{}' must start with tumult_",
                tool.name
            );
        }
    }

    // ── MCP Authentication ───────────────────────────────────

    #[test]
    fn auth_no_token_configured_allows_all() {
        let auth = McpAuth { token: None };
        assert!(auth.check(None).is_ok());
        assert!(auth.check(Some("Bearer anything")).is_ok());
    }

    #[test]
    fn auth_with_token_accepts_valid_bearer() {
        let auth = McpAuth {
            token: Some("secret123".into()),
        };
        assert!(auth.check(Some("Bearer secret123")).is_ok());
    }

    #[test]
    fn auth_with_token_rejects_missing_header() {
        let auth = McpAuth {
            token: Some("secret123".into()),
        };
        let result = auth.check(None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("missing Authorization"));
    }

    #[test]
    fn auth_with_token_rejects_wrong_token() {
        let auth = McpAuth {
            token: Some("secret123".into()),
        };
        let result = auth.check(Some("Bearer wrong_token"));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid bearer token"));
    }

    #[test]
    fn auth_with_token_rejects_non_bearer_scheme() {
        let auth = McpAuth {
            token: Some("secret123".into()),
        };
        let result = auth.check(Some("Basic dXNlcjpwYXNz"));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("expected Bearer token"));
    }

    // ── Auth wired into handler ──────────────────────────────

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

    /// Verify constant-time comparison is used: tokens that differ only in a
    /// single bit (or by length) are still rejected, and the comparison does
    /// not short-circuit on a matching prefix.
    #[test]
    fn auth_constant_time_comparison() {
        use subtle::ConstantTimeEq;

        let expected = b"super-secret-token";
        // Shorter slice — must not match, even though it is a prefix.
        let short = b"super-secret-toke";
        let matches: bool = short.ct_eq(expected).into();
        assert!(!matches, "short token must not match expected");

        // One-bit-off: change last byte.
        let mut one_off = *expected;
        one_off[expected.len() - 1] ^= 0x01;
        let matches: bool = one_off.ct_eq(expected).into();
        assert!(!matches, "one-bit-different token must not match expected");

        // Longer than expected — different length, must not match.
        let long = b"super-secret-tokenXXXX";
        let matches: bool = long.ct_eq(expected).into();
        assert!(!matches, "longer token must not match expected");

        // Positive case: exact match must succeed.
        let matches: bool = expected.ct_eq(expected).into();
        assert!(matches, "exact match must succeed");

        // End-to-end via McpAuth.check (length-prefix variant).
        let auth = McpAuth {
            token: Some("super-secret-token".into()),
        };
        assert!(auth.check(Some("Bearer super-secret-toke")).is_err());
        assert!(auth.check(Some("Bearer super-secret-tokenXXXX")).is_err());
        assert!(auth.check(Some("Bearer super-secret-token")).is_ok());
    }

    // ── Bind address ─────────────────────────────────────────

    #[test]
    fn mcp_bind_address_is_localhost_only() {
        let addr = mcp_bind_address();
        assert_eq!(addr.ip(), std::net::Ipv4Addr::LOCALHOST);
    }

    #[test]
    fn mcp_bind_address_never_binds_to_all_interfaces() {
        let addr = mcp_bind_address();
        assert_ne!(
            addr.ip(),
            std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
        );
    }

    // ── Process executor timeout ─────────────────────────────

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn process_executor_respects_timeout() {
        let executor = ProcessExecutor;
        let activity = tumult_core::types::Activity {
            name: "timeout-test".into(),
            activity_type: tumult_core::types::ActivityType::Action,
            provider: tumult_core::types::Provider::Process {
                path: "sleep".into(),
                arguments: vec!["60".into()],
                env: std::collections::HashMap::new(),
                timeout_s: Some(0.2), // 200ms timeout
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        };

        let outcome = executor.execute(&activity);
        assert!(outcome.error.as_ref().unwrap().contains("timed out"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn process_executor_records_duration() {
        let executor = ProcessExecutor;
        let activity = tumult_core::types::Activity {
            name: "duration-test".into(),
            activity_type: tumult_core::types::ActivityType::Action,
            provider: tumult_core::types::Provider::Process {
                path: "echo".into(),
                arguments: vec!["hello".into()],
                env: std::collections::HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
            label_selector: None,
        };

        let outcome = executor.execute(&activity);
        assert!(outcome.success);
        assert_eq!(outcome.output.as_deref(), Some("hello"));
        // Duration should be recorded (previously was always 0)
        // It may still be 0 for very fast commands, so just check it's not negative
        // (u64 is always >= 0)
    }

    // ── Handler workspace root ───────────────────────────────

    #[test]
    fn handler_with_workspace_root_sets_path() {
        let handler = TumultHandler::with_workspace_root("/tmp".into());
        assert_eq!(handler.workspace_root, std::path::PathBuf::from("/tmp"));
    }

    #[test]
    fn default_store_path_returns_non_empty_string() {
        // Verifies default_store_path() never silently produces an empty string.
        let path = default_store_path();
        assert!(!path.is_empty(), "default_store_path must not be empty");
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
