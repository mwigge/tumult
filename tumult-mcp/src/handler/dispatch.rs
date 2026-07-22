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
    AnalyzeStoreTool, AnalyzeTool, AutopilotExportTool, AutopilotNotifyTool, AutopilotRespondTool,
    AutopilotRunTool, AutopilotStatusTool, ChaosGraphCoverageGapsTool, ChaosGraphCypherTool,
    ChaosGraphNeighborsTool, ChaosGraphQueryTool, ComplianceLineageTool, ComplianceTool,
    CoverageTool, CreateExperimentTool, DiscoverTool, FaultCatalogTool, GameDayAnalyzeTool,
    GameDayCreateTool, GameDayListTool, GameDayRunTool, ListExperimentsTool, ListJournalsTool,
    QueryTracesTool, ReadJournalTool, RecommendInjectionTool, RecommendTool, ReportTool,
    RunExperimentTool, ScaffoldExperimentTool, StoreStatsTool, TopologyImportTool, TopologyMapTool,
    TrendTool, ValidateTool, WhoamiTool,
};
use super::{Role, TumultHandler};

// Per-tool-family dispatch bodies. Each module adds `tool_*` methods to
// `TumultHandler`; `handle_call_tool_request` below routes to them.
mod agentic;
mod analytics;
mod autopilot;
mod experiment;
mod gameday;
mod graph;
mod journal;
mod meta;
mod topology;

/// Minimum [`Role`] required to invoke a tool.
///
/// Derived from each tool's `read_only_hint` (the source of truth): a tool
/// whose hint is `true` needs [`Role::Viewer`]; everything else — fault
/// injection, execution, file writes, agent spawning — needs
/// [`Role::Operator`]. **Fail-closed**: any tool not listed here (including an
/// unknown name) requires `Operator`. The `required_roles_match_annotations`
/// test cross-checks this table against every tool's declared `read_only_hint`,
/// so the two can never silently diverge.
pub(crate) fn required_role_for_tool(name: &str) -> Role {
    match name {
        "tumult_validate"
        | "tumult_analyze"
        | "tumult_read_journal"
        | "tumult_list_journals"
        | "tumult_discover"
        | "tumult_query_traces"
        | "tumult_store_stats"
        | "tumult_analyze_store"
        | "tumult_list_experiments"
        | "tumult_compliance"
        | "tumult_trend"
        | "tumult_agents"
        | "tumult_gameday_analyze"
        | "tumult_gameday_list"
        | "tumult_coverage"
        | "tumult_agentic_list_scenarios"
        | "tumult_agentic_smoke"
        | "tumult_agentic_run_experiment"
        | "tumult_chaosgraph_query"
        | "tumult_chaosgraph_neighbors"
        | "tumult_chaosgraph_coverage_gaps"
        | "tumult_chaosgraph_cypher"
        | "tumult_fault_catalog"
        | "tumult_scaffold_experiment"
        | "tumult_topology_map"
        | "tumult_compliance_lineage"
        | "tumult_recommend_injection"
        | "tumult_autopilot_status"
        | "tumult_whoami" => Role::Viewer,
        // Operator-only (read_only_hint == false), plus any unknown tool:
        // tumult_run_experiment, tumult_create_experiment, tumult_report,
        // tumult_gameday_run, tumult_gameday_create, tumult_recommend,
        // tumult_topology_import, tumult_autopilot_run,
        // tumult_autopilot_respond, tumult_autopilot_export.
        _ => Role::Operator,
    }
}

/// Journal file name used when `tumult_run_experiment` receives no
/// `journal_path` — mirrors the CLI's `--journal-path` default.
const DEFAULT_JOURNAL_PATH: &str = "journal.toon";

/// Maximum accepted value for the list tools' `limit` parameter.
const LIST_LIMIT_MAX: u64 = 1000;

/// Result of a per-family dispatch body: the outer `Result` carries a
/// protocol-level [`CallToolError`] (argument parsing, path validation),
/// while the inner `Result` is the tool-level outcome fed to
/// [`finish_tool_call`]. Routing arms unwrap the outer `Result` with `?`.
type Dispatched =
    std::result::Result<std::result::Result<ToolOutput, crate::error::ToolError>, CallToolError>;

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

    /// Shed floods cheaply, before authentication or dispatch work: one
    /// token from the caller's bucket. The key is the MCP session id (the
    /// SDK does not expose the peer IP at any layer the handler can hook);
    /// requests without a session (stdio, pre-initialize) share one global
    /// bucket. Disabled entirely when the limiter is configured with RPS 0.
    fn check_rate_limit(
        &self,
        runtime: &Arc<dyn rust_mcp_sdk::McpServer>,
    ) -> std::result::Result<(), RpcError> {
        let client = runtime.session_id().unwrap_or_default();
        if self.rate_limiter.check(&client) {
            Ok(())
        } else {
            Err(RpcError::invalid_request()
                .with_message("rate limit exceeded: too many requests; slow down".to_string()))
        }
    }
}

/// Resolve the analytics store path for a tool call. A viewer-role caller
/// may not point `DuckDB` at arbitrary filesystem paths (store errors relay
/// host filesystem layout), so the `store_path` override is honored for
/// operators — and for open-mode local callers, who are unauthenticated by
/// design — while viewers always get the default/configured store.
pub(super) fn store_path_for(role: Option<Role>, requested: &str) -> String {
    match role {
        Some(Role::Viewer) => super::schema::default_store_path(),
        Some(Role::Operator) | None => requested.to_string(),
    }
}

#[async_trait]
impl ServerHandler for TumultHandler {
    async fn handle_list_tools_request(
        &self,
        params: Option<PaginatedRequestParams>,
        runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ListToolsResult, RpcError> {
        self.check_rate_limit(&runtime)?;
        // tools/list reveals the callable surface (including the destructive
        // tools), so it goes through the same bearer gate as resources/list:
        // when auth is configured a valid token (any role) is required; with
        // no auth configured (loopback-only mode) it stays open.
        let params = params.unwrap_or_default();
        let meta_authorization = params
            .meta
            .as_ref()
            .and_then(|m| m.extra.as_ref())
            .and_then(|extra| extra.get("authorization"))
            .and_then(|v| v.as_str())
            .map(std::string::ToString::to_string);
        let authorization = Self::resolve_authorization(meta_authorization, &runtime).await;
        self.auth
            .check(authorization.as_deref())
            .map_err(|e| RpcError::invalid_request().with_message(format!("Unauthorized: {e}")))?;
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
            ChaosGraphQueryTool::tool(),
            ChaosGraphNeighborsTool::tool(),
            ChaosGraphCoverageGapsTool::tool(),
            ChaosGraphCypherTool::tool(),
            FaultCatalogTool::tool(),
            ScaffoldExperimentTool::tool(),
            TopologyImportTool::tool(),
            TopologyMapTool::tool(),
            ComplianceLineageTool::tool(),
            RecommendInjectionTool::tool(),
            AutopilotRunTool::tool(),
            AutopilotStatusTool::tool(),
            AutopilotRespondTool::tool(),
            AutopilotExportTool::tool(),
            AutopilotNotifyTool::tool(),
            WhoamiTool::tool(),
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

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        // Shed floods cheaply, before any auth or dispatch work.
        self.check_rate_limit(&runtime)
            .map_err(|e| CallToolError::from_message(e.message))?;

        // Enforce bearer-token authentication and role-based access control.
        // The Authorization value arrives either via `_meta.authorization`
        // (stdio, or explicit override) or via the HTTP `Authorization:
        // Bearer` header captured on the session runtime; explicit `_meta`
        // takes precedence when both are present.
        let authorization =
            Self::resolve_authorization(Self::extract_authorization(&params), &runtime).await;
        let principal_role = match self.auth.authenticate(authorization.as_deref()) {
            Ok(role) => role,
            Err(e) => return Err(CallToolError::from_message(format!("Unauthorized: {e}"))),
        };
        // `None` == open mode (no auth configured) → full access. Otherwise the
        // resolved role must meet the tool's required role (Operator ⊇ Viewer).
        if let Some(role) = principal_role {
            let required = required_role_for_tool(&params.name);
            if role < required {
                return Err(CallToolError::from_message(format!(
                    "Unauthorized: tool {} requires the '{}' role; token has '{}'",
                    params.name,
                    required.as_str(),
                    role.as_str()
                )));
            }
        }

        // Acquire the concurrency permit only after the caller is
        // authenticated and authorized — unauthenticated work must not hold
        // execution slots.
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| CallToolError::from_message("semaphore closed".to_string()))?;

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
            "tumult_run_experiment" => experiment::run_experiment(self, &params, mcp_context)?,
            "tumult_validate" => experiment::validate(self, &params)?,
            "tumult_analyze" => analytics::analyze(self, &params)?,
            "tumult_read_journal" => journal::read_journal(self, &params)?,
            "tumult_list_journals" => journal::list_journals(self, &params)?,
            "tumult_discover" => meta::discover(),
            "tumult_create_experiment" => experiment::create_experiment(self, &params)?,
            "tumult_query_traces" => journal::query_traces(self, &params)?,
            "tumult_store_stats" => analytics::store_stats(&params, principal_role)?,
            "tumult_analyze_store" => analytics::analyze_store(&params, principal_role)?,
            "tumult_list_experiments" => experiment::list_experiments(self, &params)?,
            "tumult_report" => journal::report(self, &params)?,
            "tumult_compliance" => analytics::compliance(self, &params)?,
            "tumult_trend" => analytics::trend(self, &params)?,
            "tumult_agents" => agentic::agents(&params)?,
            "tumult_gameday_create" => gameday::gameday_create(self, &params)?,
            "tumult_gameday_run" => gameday::gameday_run(self, &params)?,
            "tumult_gameday_analyze" => gameday::gameday_analyze(self, &params)?,
            "tumult_gameday_list" => gameday::gameday_list(self, &params)?,
            "tumult_recommend" => analytics::recommend(self, &params)?,
            "tumult_coverage" => analytics::coverage(&params)?,
            "tumult_agentic_list_scenarios" => agentic::agentic_list_scenarios(&params)?,
            "tumult_agentic_smoke" => agentic::agentic_smoke(&params)?,
            "tumult_agentic_run_experiment" => agentic::agentic_run_experiment(&params)?,
            "tumult_chaosgraph_query" => graph::chaosgraph_query(&params)?,
            "tumult_chaosgraph_neighbors" => graph::chaosgraph_neighbors(&params)?,
            "tumult_chaosgraph_coverage_gaps" => graph::chaosgraph_coverage_gaps(&params)?,
            "tumult_chaosgraph_cypher" => graph::chaosgraph_cypher(&params, principal_role)?,
            "tumult_fault_catalog" => meta::fault_catalog(&params)?,
            "tumult_scaffold_experiment" => experiment::scaffold_experiment(&params)?,
            "tumult_topology_import" => topology::topology_import(&params)?,
            "tumult_topology_map" => topology::topology_map(&params)?,
            "tumult_compliance_lineage" => topology::compliance_lineage(&params)?,
            "tumult_recommend_injection" => topology::recommend_injection(&params)?,
            "tumult_autopilot_run" => autopilot::autopilot_run(self, &params)?,
            "tumult_autopilot_status" => autopilot::autopilot_status(&params, principal_role)?,
            "tumult_autopilot_respond" => autopilot::autopilot_respond(self, &params)?,
            "tumult_autopilot_export" => autopilot::autopilot_export(&params)?,
            "tumult_autopilot_notify" => autopilot::autopilot_notify(&params)?,
            "tumult_whoami" => meta::whoami(&params, principal_role)?,
            _ => return Err(CallToolError::unknown_tool(params.name)),
        };

        Ok(finish_tool_call(&params.name, result))
    }

    async fn handle_list_resources_request(
        &self,
        params: Option<PaginatedRequestParams>,
        runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ListResourcesResult, RpcError> {
        self.check_rate_limit(&runtime)?;
        let params = params.unwrap_or_default();
        // Resources go through the same bearer gate as tool calls.
        let authorization = Self::resolve_authorization(
            params
                .meta
                .as_ref()
                .and_then(|m| m.extra.as_ref())
                .and_then(|extra| extra.get("authorization"))
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            &runtime,
        )
        .await;
        self.check_resource_auth(authorization.as_deref())?;
        tokio::task::block_in_place(|| self.list_resources_page(params.cursor.as_deref()))
    }

    async fn handle_read_resource_request(
        &self,
        params: ReadResourceRequestParams,
        runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ReadResourceResult, RpcError> {
        self.check_rate_limit(&runtime)?;
        let authorization = Self::resolve_authorization(
            params
                .meta
                .as_ref()
                .and_then(|m| m.extra.as_ref())
                .and_then(|extra| extra.get("authorization"))
                .and_then(|v| v.as_str())
                .map(std::string::ToString::to_string),
            &runtime,
        )
        .await;
        self.check_resource_auth(authorization.as_deref())?;
        tokio::task::block_in_place(|| self.read_resource_uri(&params.uri))
    }
}

/// Convert a dispatched tool outcome into a `CallToolResult`, emitting the
/// completion/error telemetry event.
fn finish_tool_call(
    tool_name: &str,
    result: std::result::Result<ToolOutput, crate::error::ToolError>,
) -> CallToolResult {
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
            result
        }
        Err(e) => {
            crate::telemetry::event_tool_error(tool_name, &e.to_string());
            // Per MCP spec, tool-level failures are reported inside the
            // result with `isError: true`, not as protocol errors.
            let mut result = CallToolResult::text_content(vec![format!("Error: {e}").into()]);
            result.is_error = Some(true);
            result
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
mod tests;
