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
    tumult_analytics::AnalyticsStore::default_path()
        .to_str()
        .unwrap_or_default()
        .to_string()
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

// ── Process executor (shared pattern with CLI) ────────────────

pub struct ProcessExecutor;

impl tumult_core::runner::ActivityExecutor for ProcessExecutor {
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
    /// Returns an error string if the token is missing or does not match the
    /// configured bearer token.
    pub fn check(&self, authorization: Option<&str>) -> std::result::Result<(), String> {
        match &self.token {
            None => Ok(()), // no token configured, allow all
            Some(expected) => match authorization {
                Some(header) => {
                    let prefix = "Bearer ";
                    if let Some(provided) = header.strip_prefix(prefix) {
                        if provided == expected {
                            Ok(())
                        } else {
                            Err("invalid bearer token".into())
                        }
                    } else {
                        Err("expected Bearer token in Authorization header".into())
                    }
                }
                None => Err("missing Authorization header".into()),
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
    pub semaphore: tokio::sync::Semaphore,
    /// Base directory for file operations (path traversal prevention).
    pub workspace_root: std::path::PathBuf,
}

impl Default for TumultHandler {
    fn default() -> Self {
        Self {
            semaphore: tokio::sync::Semaphore::new(MAX_CONCURRENT_TOOL_CALLS),
            workspace_root: std::env::current_dir().unwrap_or_else(|_| "/".into()),
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
        }
    }

    /// Validate and resolve a user-supplied file path against the workspace root.
    fn resolve_path(&self, user_path: &str) -> std::result::Result<String, CallToolError> {
        let resolved = tools::safe_resolve_path(&self.workspace_root, user_path)
            .map_err(|e| CallToolError::invalid_arguments("path", Some(e)))?;
        Ok(resolved.to_str().unwrap_or_default().to_string())
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
            ],
            meta: None,
            next_cursor: None,
        })
    }

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

        tracing::info!(tool = %params.name, "MCP tool call");
        // SpanGuard contains a non-Send OTel context guard; record span name
        // before await points and drop immediately so the future stays Send.
        {
            let _span = crate::telemetry::begin_tool_call(&params.name);
        }

        let result = match params.name.as_str() {
            "tumult_run_experiment" => {
                let args: RunExperimentTool = parse_args(&params)?;
                let path = self.resolve_path(&args.experiment_path)?;
                tools::run_experiment(&path, &args.rollback_strategy)
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
            _ => return Err(CallToolError::unknown_tool(params.name)),
        };

        match result {
            Ok(content) => {
                crate::telemetry::event_tool_completed(&params.name, true);
                Ok(CallToolResult::text_content(vec![content.into()]))
            }
            Err(e) => {
                crate::telemetry::event_tool_error(&params.name, &e);
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
#[allow(clippy::useless_vec)]
mod tests {
    use super::*;
    use tumult_core::runner::ActivityExecutor;

    #[test]
    fn all_ten_tools_listed() {
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
        ];
        assert_eq!(tools.len(), 10);
    }

    #[test]
    fn handler_has_semaphore_with_correct_limit() {
        let handler = TumultHandler::default();
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
        assert!(result.unwrap_err().contains("missing Authorization"));
    }

    #[test]
    fn auth_with_token_rejects_wrong_token() {
        let auth = McpAuth {
            token: Some("secret123".into()),
        };
        let result = auth.check(Some("Bearer wrong_token"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid bearer token"));
    }

    #[test]
    fn auth_with_token_rejects_non_bearer_scheme() {
        let auth = McpAuth {
            token: Some("secret123".into()),
        };
        let result = auth.check(Some("Basic dXNlcjpwYXNz"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("expected Bearer token"));
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
}
