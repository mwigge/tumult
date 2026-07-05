//! Live MCP client over Streamable-HTTP: connection/transport plumbing
//! (`post`/`handshake`) plus one method per tool endpoint the panel drives.
//! Also defines [`ScaffoldArgs`] (the scaffold request shape) and the
//! [`ChaosLoopClient`] abstraction the orchestration is unit-tested against.

use std::time::Duration;

use serde_json::{json, Value};

use super::parse::{
    parse_analyze_store_result, parse_catalog_result, parse_compliance_result,
    parse_coverage_gaps_result, parse_discover_result, parse_graph_neighbors_result,
    parse_graph_query_result, parse_recommend_result, parse_run_result, parse_scaffold_result,
    parse_tools_list, parse_validate_result, parse_whoami_result, CatalogOutcome,
    ComplianceOutcome, CoverageGapsOutcome, DiscoverOutcome, GraphEgoOutcome, GraphNodesOutcome,
    RecommendOutcome, RunOutcome, ScaffoldOutcome, TableOutcome, ToolInfo, ValidateOutcome,
    WhoamiOutcome,
};
use super::protocol::{
    build_notification, build_rpc_request, initialize_params, parse_sse_body, rpc_result,
    tools_call_params, McpError,
};

/// MCP JSON-RPC endpoint path appended to the configured base URL.
const MCP_PATH: &str = "/mcp";

/// Inputs for a `tumult_scaffold_experiment` call, mirroring the tool's
/// argument schema. Empty optionals are omitted from the request.
#[derive(Debug, Clone, Default)]
pub struct ScaffoldArgs {
    /// Owning plugin (e.g. `tumult-network`). Optional when `action` is a
    /// fully-qualified `plugin::action`.
    pub plugin: Option<String>,
    /// Action name, or `plugin::action`.
    pub action: String,
    /// Argument values as a JSON object.
    pub args: Value,
    /// Logical target of the fault.
    pub target: String,
    /// Shell command for the steady-state probe (mutually exclusive with
    /// `probe_url`).
    pub probe_command: Option<String>,
    /// HTTP URL for the steady-state probe.
    pub probe_url: Option<String>,
    /// Regex the probe output/response must match.
    pub probe_expect: Option<String>,
    /// Experiment title (defaults server-side to `<action> — <target>`).
    pub title: Option<String>,
}

// ── Live client ───────────────────────────────────────────────

/// A stateless MCP client. Each high-level call performs a fresh
/// initialize→operate handshake, so there is no session state to expire on our
/// side — robust for a demo where the MCP server may restart underneath us.
#[derive(Clone)]
pub struct McpClient {
    http: reqwest::Client,
    endpoint: String,
    token: Option<String>,
}

impl McpClient {
    /// Build a client for `base_url` (e.g. `http://tumult-mcp:3100`). The MCP
    /// path is appended automatically. `token` is sent as a bearer token when
    /// non-empty.
    #[must_use]
    pub fn new(base_url: &str, token: Option<String>) -> Self {
        let base = base_url.trim_end_matches('/');
        let endpoint = format!("{base}{MCP_PATH}");
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        Self {
            http,
            endpoint,
            token: token.filter(|t| !t.is_empty()),
        }
    }

    /// POST one JSON-RPC message. Returns the raw response body plus any
    /// `mcp-session-id` header the server set.
    async fn post(
        &self,
        session: Option<&str>,
        payload: &Value,
    ) -> Result<(String, Option<String>), McpError> {
        let mut req = self
            .http
            .post(&self.endpoint)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .json(payload);
        if let Some(t) = &self.token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        if let Some(s) = session {
            req = req.header("mcp-session-id", s);
        }

        let resp = req.send().await.map_err(|e| {
            if e.is_connect() || e.is_timeout() {
                McpError::Unreachable(e.to_string())
            } else {
                McpError::Transport(e.to_string())
            }
        })?;

        let session_id = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
            .map(ToString::to_string);
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| McpError::Transport(e.to_string()))?;

        if !status.is_success() {
            return Err(McpError::Transport(format!(
                "HTTP {status} from MCP server: {}",
                body.chars().take(200).collect::<String>()
            )));
        }
        Ok((body, session_id))
    }

    /// Perform `initialize` and return the session id the server assigned.
    async fn handshake(&self) -> Result<String, McpError> {
        let init = build_rpc_request(1, "initialize", initialize_params());
        let (body, session) = self.post(None, &init).await?;
        // Surface any JSON-RPC error the server returned on initialize.
        rpc_result(&parse_sse_body(&body)?)?;
        let session = session
            .ok_or_else(|| McpError::Transport("server did not return an mcp-session-id".into()))?;
        // Best-effort: tell the server we're ready. Failure here is non-fatal.
        let note = build_notification("notifications/initialized", json!({}));
        let _ = self.post(Some(&session), &note).await;
        Ok(session)
    }

    /// List tools, reading annotations. Full handshake each call.
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport or protocol failure.
    pub async fn list_tools(&self) -> Result<Vec<ToolInfo>, McpError> {
        let session = self.handshake().await?;
        let req = build_rpc_request(2, "tools/list", json!({}));
        let (body, _) = self.post(Some(&session), &req).await?;
        let result = rpc_result(&parse_sse_body(&body)?)?;
        Ok(parse_tools_list(&result))
    }

    /// Call a tool by name with the given arguments, returning the raw
    /// `tools/call` result object. Performs a fresh handshake each call.
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport or RPC-level failure.
    async fn call_tool(&self, name: &str, arguments: Value) -> Result<Value, McpError> {
        let session = self.handshake().await?;
        let mut params = tools_call_params(name, arguments);
        // The server reads the tool-call token from `_meta.authorization` (the
        // transport Authorization header is not visible at the handler level in
        // rust-mcp-sdk). Setting the header alone authenticates `tools/list` but
        // not `tools/call`, so inject it into the params too.
        if let Some(t) = &self.token {
            params["_meta"] = json!({ "authorization": format!("Bearer {t}") });
        }
        let req = build_rpc_request(3, "tools/call", params);
        let (body, _) = self.post(Some(&session), &req).await?;
        rpc_result(&parse_sse_body(&body)?)
    }

    /// Run an experiment by path via `tumult_run_experiment`.
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport, RPC, or tool-level failure.
    pub async fn run_experiment(&self, experiment_path: &str) -> Result<RunOutcome, McpError> {
        let result = self
            .call_tool(
                "tumult_run_experiment",
                json!({ "experiment_path": experiment_path }),
            )
            .await?;
        parse_run_result(&result)
    }

    /// List every plugin/action the server can dispatch via `tumult_discover`.
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport, RPC, or tool-level failure.
    pub async fn discover(&self) -> Result<DiscoverOutcome, McpError> {
        let result = self.call_tool("tumult_discover", json!({})).await?;
        parse_discover_result(&result)
    }

    /// Validate an experiment file via `tumult_validate`.
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport, RPC, or tool-level failure
    /// (an invalid experiment surfaces as a tool-level [`McpError::Protocol`]).
    pub async fn validate(&self, experiment_path: &str) -> Result<ValidateOutcome, McpError> {
        let result = self
            .call_tool(
                "tumult_validate",
                json!({ "experiment_path": experiment_path }),
            )
            .await?;
        parse_validate_result(&result)
    }

    /// Run a read-only SQL query over the persistent analytics store via
    /// `tumult_analyze_store` (store path defaults server-side).
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport, RPC, or tool-level failure.
    pub async fn analyze_store(&self, query: &str) -> Result<TableOutcome, McpError> {
        let result = self
            .call_tool("tumult_analyze_store", json!({ "query": query }))
            .await?;
        parse_analyze_store_result(&result)
    }

    /// Ask what to test next via `tumult_recommend` (store path defaults
    /// server-side).
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport, RPC, or tool-level failure.
    pub async fn recommend(&self) -> Result<RecommendOutcome, McpError> {
        let result = self.call_tool("tumult_recommend", json!({})).await?;
        parse_recommend_result(&result)
    }

    /// Summarise regulatory evidence over a directory of journals via
    /// `tumult_compliance`.
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport, RPC, or tool-level failure.
    pub async fn compliance(
        &self,
        framework: &str,
        journals_path: &str,
    ) -> Result<ComplianceOutcome, McpError> {
        let result = self
            .call_tool(
                "tumult_compliance",
                json!({ "framework": framework, "journals_path": journals_path }),
            )
            .await?;
        parse_compliance_result(&result)
    }

    /// List ChaosGraph nodes of a `kind` via `tumult_chaosgraph_query`.
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport, RPC, or tool-level failure.
    pub async fn chaosgraph_query(
        &self,
        kind: &str,
        filter: Option<&str>,
    ) -> Result<GraphNodesOutcome, McpError> {
        let mut args = json!({ "kind": kind });
        if let Some(f) = filter {
            args["filter"] = json!(f);
        }
        let result = self.call_tool("tumult_chaosgraph_query", args).await?;
        parse_graph_query_result(&result)
    }

    /// Fetch the ego sub-graph of a node via `tumult_chaosgraph_neighbors`.
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport, RPC, or tool-level failure.
    pub async fn chaosgraph_neighbors(
        &self,
        node_id: &str,
        rel: Option<&str>,
        depth: u32,
    ) -> Result<GraphEgoOutcome, McpError> {
        let mut args = json!({ "node_id": node_id, "depth": depth });
        if let Some(r) = rel {
            args["rel"] = json!(r);
        }
        let result = self.call_tool("tumult_chaosgraph_neighbors", args).await?;
        parse_graph_neighbors_result(&result)
    }

    /// List untested actions (and, with a framework, unevidenced articles) via
    /// `tumult_chaosgraph_coverage_gaps`.
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport, RPC, or tool-level failure.
    pub async fn chaosgraph_coverage_gaps(
        &self,
        framework: Option<&str>,
        domain: Option<&str>,
    ) -> Result<CoverageGapsOutcome, McpError> {
        let mut args = json!({});
        if let Some(f) = framework {
            args["framework"] = json!(f);
        }
        if let Some(d) = domain {
            args["domain"] = json!(d);
        }
        let result = self
            .call_tool("tumult_chaosgraph_coverage_gaps", args)
            .await?;
        parse_coverage_gaps_result(&result)
    }

    /// Fetch the live fault catalog (domains → actions → args) via
    /// `tumult_fault_catalog` — what the "New experiment" picker populates from.
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport, RPC, or tool-level failure.
    pub async fn fault_catalog(&self) -> Result<CatalogOutcome, McpError> {
        let result = self.call_tool("tumult_fault_catalog", json!({})).await?;
        parse_catalog_result(&result)
    }

    /// Scaffold an experiment from a chosen action via
    /// `tumult_scaffold_experiment`, returning the generated TOON and whether it
    /// validates. Empty optional fields are omitted from the request.
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport, RPC, or tool-level failure. An
    /// experiment that scaffolds but fails validation is NOT an error: it comes
    /// back as [`ScaffoldOutcome`] with `valid: false`.
    pub async fn scaffold_experiment(
        &self,
        req: ScaffoldArgs,
    ) -> Result<ScaffoldOutcome, McpError> {
        let mut args = json!({
            "action": req.action,
            "target": req.target,
            "args": if req.args.is_null() { json!({}) } else { req.args },
        });
        if let Some(p) = req.plugin.filter(|s| !s.is_empty()) {
            args["plugin"] = json!(p);
        }
        if let Some(c) = req.probe_command.filter(|s| !s.is_empty()) {
            args["probe_command"] = json!(c);
        }
        if let Some(u) = req.probe_url.filter(|s| !s.is_empty()) {
            args["probe_url"] = json!(u);
        }
        if let Some(e) = req.probe_expect.filter(|s| !s.is_empty()) {
            args["probe_expect"] = json!(e);
        }
        if let Some(t) = req.title.filter(|s| !s.is_empty()) {
            args["title"] = json!(t);
        }
        let result = self.call_tool("tumult_scaffold_experiment", args).await?;
        parse_scaffold_result(&result)
    }

    /// Ask the server who the panel is authenticated as via `tumult_whoami` —
    /// the caller's resolved RBAC role and whether the request carried a valid
    /// token. Read-only and viewer-callable; the UI uses it to enforce the same
    /// tiers the server enforces (defense in depth).
    ///
    /// # Errors
    /// Propagates [`McpError`] on any transport, RPC, or tool-level failure. The
    /// HTTP layer degrades a failure to least privilege (viewer).
    pub async fn whoami(&self) -> Result<WhoamiOutcome, McpError> {
        let result = self.call_tool("tumult_whoami", json!({})).await?;
        parse_whoami_result(&result)
    }
}

/// The five MCP calls the chaos-loop showcase drives, abstracted so the
/// orchestration can be unit-tested against a mock client. Every method is one
/// `tools/call` over MCP — exactly what an autonomous agent would issue.
pub trait ChaosLoopClient {
    /// `tumult_discover`.
    fn discover(
        &self,
    ) -> impl std::future::Future<Output = Result<DiscoverOutcome, McpError>> + Send;
    /// `tumult_validate`.
    fn validate(
        &self,
        experiment_path: &str,
    ) -> impl std::future::Future<Output = Result<ValidateOutcome, McpError>> + Send;
    /// `tumult_run_experiment`.
    fn run_experiment(
        &self,
        experiment_path: &str,
    ) -> impl std::future::Future<Output = Result<RunOutcome, McpError>> + Send;
    /// `tumult_analyze_store`.
    fn analyze_store(
        &self,
        query: &str,
    ) -> impl std::future::Future<Output = Result<TableOutcome, McpError>> + Send;
    /// `tumult_recommend`.
    fn recommend(
        &self,
    ) -> impl std::future::Future<Output = Result<RecommendOutcome, McpError>> + Send;
}

impl ChaosLoopClient for McpClient {
    fn discover(
        &self,
    ) -> impl std::future::Future<Output = Result<DiscoverOutcome, McpError>> + Send {
        McpClient::discover(self)
    }
    fn validate(
        &self,
        experiment_path: &str,
    ) -> impl std::future::Future<Output = Result<ValidateOutcome, McpError>> + Send {
        McpClient::validate(self, experiment_path)
    }
    fn run_experiment(
        &self,
        experiment_path: &str,
    ) -> impl std::future::Future<Output = Result<RunOutcome, McpError>> + Send {
        McpClient::run_experiment(self, experiment_path)
    }
    fn analyze_store(
        &self,
        query: &str,
    ) -> impl std::future::Future<Output = Result<TableOutcome, McpError>> + Send {
        McpClient::analyze_store(self, query)
    }
    fn recommend(
        &self,
    ) -> impl std::future::Future<Output = Result<RecommendOutcome, McpError>> + Send {
        McpClient::recommend(self)
    }
}
