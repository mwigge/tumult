//! MCP handler — routes tool calls to implementations.

use std::sync::Arc;

use async_trait::async_trait;
use rust_mcp_sdk::{macros, mcp_server::ServerHandler, schema::*, McpServer};

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
                ..
            } => {
                let output = std::process::Command::new(path)
                    .args(arguments)
                    .envs(env)
                    .output();
                match output {
                    Ok(o) => tumult_core::runner::ActivityOutcome {
                        success: o.status.success(),
                        output: Some(String::from_utf8_lossy(&o.stdout).trim().to_string()),
                        error: if o.stderr.is_empty() {
                            None
                        } else {
                            Some(String::from_utf8_lossy(&o.stderr).trim().to_string())
                        },
                        duration_ms: 0,
                    },
                    Err(e) => tumult_core::runner::ActivityOutcome {
                        success: false,
                        output: None,
                        error: Some(e.to_string()),
                        duration_ms: 0,
                    },
                }
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

// ── MCP Handler ───────────────────────────────────────────────

/// Maximum concurrent tool calls allowed.
const MAX_CONCURRENT_TOOL_CALLS: usize = 10;

pub struct TumultHandler {
    /// Semaphore limiting concurrent tool execution.
    pub semaphore: tokio::sync::Semaphore,
}

impl Default for TumultHandler {
    fn default() -> Self {
        Self {
            semaphore: tokio::sync::Semaphore::new(MAX_CONCURRENT_TOOL_CALLS),
        }
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
        let _span = crate::telemetry::begin_tool_call(&params.name);

        let result = match params.name.as_str() {
            "tumult_run_experiment" => {
                let args: RunExperimentTool = parse_args(&params)?;
                tools::run_experiment(&args.experiment_path, &args.rollback_strategy)
            }
            "tumult_validate" => {
                let args: ValidateTool = parse_args(&params)?;
                tools::validate_experiment(&args.experiment_path)
            }
            "tumult_analyze" => {
                let args: AnalyzeTool = parse_args(&params)?;
                tools::analyze(&args.journals_path, &args.query)
            }
            "tumult_read_journal" => {
                let args: ReadJournalTool = parse_args(&params)?;
                tools::read_journal(&args.journal_path)
            }
            "tumult_list_journals" => {
                let args: ListJournalsTool = parse_args(&params)?;
                tools::list_journals(&args.directory).map(|v| v.join("\n"))
            }
            "tumult_discover" => Ok(tools::discover_plugins()),
            "tumult_create_experiment" => {
                let args: CreateExperimentTool = parse_args(&params)?;
                tools::create_experiment(&args.output_path, args.plugin.as_deref())
            }
            "tumult_query_traces" => {
                let args: QueryTracesTool = parse_args(&params)?;
                tools::query_traces(&args.journal_path)
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
                    format!("Error: {}", e).into()
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
}
