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

#[derive(Default)]
pub struct TumultHandler;

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
        tracing::info!(tool = %params.name, "MCP tool call");

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
            _ => return Err(CallToolError::unknown_tool(params.name)),
        };

        match result {
            Ok(content) => Ok(CallToolResult::text_content(vec![content.into()])),
            Err(e) => Ok(CallToolResult::text_content(vec![
                format!("Error: {}", e).into()
            ])),
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
    fn all_seven_tools_listed() {
        // Verify we expose exactly 7 tools
        let tools = vec![
            RunExperimentTool::tool(),
            ValidateTool::tool(),
            AnalyzeTool::tool(),
            ReadJournalTool::tool(),
            ListJournalsTool::tool(),
            DiscoverTool::tool(),
            CreateExperimentTool::tool(),
        ];
        assert_eq!(tools.len(), 7);
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
