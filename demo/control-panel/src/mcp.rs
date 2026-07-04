//! Minimal MCP (Model Context Protocol) client over the Streamable-HTTP
//! transport used by `tumult-mcp --transport http`.
//!
//! The transport speaks JSON-RPC 2.0 framed as Server-Sent Events. A request
//! is `POST`ed to `<base>/mcp`; the response body is an SSE stream whose
//! `data:` lines carry the JSON-RPC payload. The server mints a session id in
//! the `mcp-session-id` response header on `initialize`, which every
//! subsequent request must echo back.
//!
//! Everything in this module that parses or builds protocol messages is a pure
//! function with unit tests against canned JSON — no live server required.

use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

/// MCP JSON-RPC endpoint path appended to the configured base URL.
const MCP_PATH: &str = "/mcp";

/// Protocol version we advertise on `initialize`.
const PROTOCOL_VERSION: &str = "2025-11-25";

/// A tool as reported by `tools/list`, reduced to the fields the panel needs.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// `annotations.destructiveHint` — true means the tool performs a
    /// destructive/irreversible action (fault injection) and the UI should
    /// require an explicit confirmation before calling it.
    pub destructive: bool,
    /// `annotations.readOnlyHint`.
    pub read_only: bool,
}

/// Normalised outcome of a `tumult_run_experiment` call.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RunOutcome {
    /// Raw journal status: completed / deviated / aborted / failed / interrupted
    /// / halted (the auto-halt guard pulled the run mid-flight).
    pub status: String,
    /// UI-facing verdict derived from `status`:
    /// "passed" | "failed" | "deviated" | "halted".
    pub outcome: String,
    pub duration_ms: Option<u64>,
    pub journal_path: Option<String>,
    pub ingestion: Option<String>,
}

/// Result of a `tumult_discover` call: how many plugins and actions the server
/// can dispatch to. Parsed from the tool's text output (discover advertises no
/// structured schema).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiscoverOutcome {
    pub plugins: usize,
    pub actions: usize,
}

/// Result of a `tumult_validate` call. Parsed from the tool's text summary
/// (`Valid: '<title>' — N method steps, M rollbacks`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ValidateOutcome {
    pub valid: bool,
    pub title: Option<String>,
    pub method_steps: usize,
    pub rollbacks: usize,
    /// The raw one-line summary the tool returned.
    pub summary: String,
}

/// A tabular result from `tumult_analyze_store` (tab-separated text output).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TableOutcome {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub row_count: usize,
}

/// One recommendation from `tumult_recommend`'s structured content.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Recommendation {
    pub rank: i64,
    pub title: String,
    pub rationale: String,
}

/// Result of a `tumult_recommend` call. Either a `message` (no analytics store
/// yet) or a ranked list of recommendations from the structured content.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecommendOutcome {
    pub message: Option<String>,
    pub recommendations: Vec<Recommendation>,
}

/// Errors surfaced to the HTTP layer. Every variant maps to a clean JSON error
/// response — the panel never panics on a bad/absent MCP server.
#[derive(Debug)]
pub enum McpError {
    /// Could not reach the server (connection refused, DNS, timeout, …).
    Unreachable(String),
    /// Transport/HTTP-level failure (non-2xx, missing session header, …).
    Transport(String),
    /// The server returned a JSON-RPC `error` object.
    Rpc(String),
    /// The tool itself reported failure (`isError: true`) or an unparseable result.
    Protocol(String),
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpError::Unreachable(m) => write!(f, "MCP unreachable: {m}"),
            McpError::Transport(m) => write!(f, "MCP transport error: {m}"),
            McpError::Rpc(m) => write!(f, "MCP error: {m}"),
            McpError::Protocol(m) => write!(f, "MCP protocol error: {m}"),
        }
    }
}

impl std::error::Error for McpError {}

// ── Pure protocol helpers (unit-tested) ───────────────────────

/// Build a JSON-RPC 2.0 request envelope.
#[must_use]
pub fn build_rpc_request(id: u64, method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    })
}

/// Build a JSON-RPC 2.0 notification (no `id`, no response expected).
#[must_use]
pub fn build_notification(method: &str, params: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    })
}

/// `initialize` params advertising our client identity and protocol version.
#[must_use]
pub fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "demo-control-panel", "version": env!("CARGO_PKG_VERSION") },
    })
}

/// `tools/call` params for a named tool with the given arguments object.
#[must_use]
pub fn tools_call_params(name: &str, arguments: Value) -> Value {
    json!({ "name": name, "arguments": arguments })
}

/// Extract the JSON-RPC payload from a Streamable-HTTP response body.
///
/// The body is either raw JSON (`application/json`) or an SSE stream where the
/// payload rides on one or more `data:` lines. We return the first `data:`
/// object that is a JSON-RPC message; if the body is plain JSON we parse it
/// directly.
///
/// # Errors
/// Returns [`McpError::Transport`] when no JSON object can be recovered.
pub fn parse_sse_body(body: &str) -> Result<Value, McpError> {
    // Fast path: the whole body is a single JSON document.
    let trimmed = body.trim_start();
    if trimmed.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
            return Ok(v);
        }
    }

    // SSE path: concatenate consecutive `data:` lines per event and parse.
    let mut data_buf = String::new();
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            if !data_buf.is_empty() {
                data_buf.push('\n');
            }
            data_buf.push_str(rest);
        } else if line.is_empty() && !data_buf.is_empty() {
            // Event boundary — try to parse what we have.
            if let Ok(v) = serde_json::from_str::<Value>(data_buf.trim()) {
                return Ok(v);
            }
            data_buf.clear();
        }
    }
    if !data_buf.is_empty() {
        if let Ok(v) = serde_json::from_str::<Value>(data_buf.trim()) {
            return Ok(v);
        }
    }
    Err(McpError::Transport(format!(
        "no JSON-RPC payload found in response body ({} bytes)",
        body.len()
    )))
}

/// Pull the `result` object out of a JSON-RPC envelope, mapping a JSON-RPC
/// `error` into [`McpError::Rpc`].
///
/// # Errors
/// Returns [`McpError::Rpc`] on a JSON-RPC error, or [`McpError::Protocol`] if
/// neither `result` nor `error` is present.
pub fn rpc_result(envelope: &Value) -> Result<Value, McpError> {
    if let Some(err) = envelope.get("error") {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        let code = err.get("code").and_then(Value::as_i64).unwrap_or(0);
        return Err(McpError::Rpc(format!("{msg} (code {code})")));
    }
    envelope
        .get("result")
        .cloned()
        .ok_or_else(|| McpError::Protocol("response had neither result nor error".into()))
}

/// Parse a `tools/list` result into [`ToolInfo`]s, reading each tool's
/// `annotations` for the destructive/read-only hints.
#[must_use]
pub fn parse_tools_list(result: &Value) -> Vec<ToolInfo> {
    result
        .get("tools")
        .and_then(Value::as_array)
        .map(|tools| {
            tools
                .iter()
                .map(|t| {
                    let ann = t.get("annotations");
                    ToolInfo {
                        name: t
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        description: t
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        destructive: ann
                            .and_then(|a| a.get("destructiveHint"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        read_only: ann
                            .and_then(|a| a.get("readOnlyHint"))
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a `tumult_run_experiment` `tools/call` result into a [`RunOutcome`].
///
/// Reads `structuredContent.journal.{status,duration_ms}`,
/// `structuredContent.journal_path`, and `structuredContent.ingestion`.
///
/// # Errors
/// Returns [`McpError::Protocol`] when the tool reported `isError: true` (the
/// error text is lifted from the `content` array) or when no journal status can
/// be found.
pub fn parse_run_result(result: &Value) -> Result<RunOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(content_text(result).unwrap_or_else(
            || "experiment tool reported an error".to_string(),
        )));
    }

    let sc = result.get("structuredContent").ok_or_else(|| {
        McpError::Protocol("run result missing structuredContent".to_string())
    })?;
    let journal = sc.get("journal").unwrap_or(sc);

    let status = journal
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Protocol("journal missing status".to_string()))?
        .to_string();

    let duration_ms = journal.get("duration_ms").and_then(Value::as_u64);
    let journal_path = sc
        .get("journal_path")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let ingestion = sc
        .get("ingestion")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Ok(RunOutcome {
        outcome: verdict_for(&status).to_string(),
        status,
        duration_ms,
        journal_path,
        ingestion,
    })
}

/// Map a raw journal status to the panel's verdict. `halted` (auto-halt guard)
/// gets its own verdict so the UI can badge it distinctly from an outright
/// failure.
#[must_use]
pub fn verdict_for(status: &str) -> &'static str {
    match status {
        "completed" => "passed",
        "deviated" => "deviated",
        "halted" => "halted",
        _ => "failed",
    }
}

// ── Discover / validate / analyze_store / recommend parsers ────

/// Parse a `tumult_discover` `tools/call` result into a [`DiscoverOutcome`].
///
/// Discover advertises no structured schema, so we read the text content and
/// pull the `Plugins: N` / `Actions: M` header counts.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or when the counts
/// cannot be located.
pub fn parse_discover_result(result: &Value) -> Result<DiscoverOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(
            content_text(result).unwrap_or_else(|| "discover tool reported an error".to_string()),
        ));
    }
    let text = content_text(result)
        .ok_or_else(|| McpError::Protocol("discover result had no text content".to_string()))?;
    let plugins = labeled_count(&text, "Plugins:")
        .ok_or_else(|| McpError::Protocol("discover output missing plugin count".to_string()))?;
    let actions = labeled_count(&text, "Actions:")
        .ok_or_else(|| McpError::Protocol("discover output missing action count".to_string()))?;
    Ok(DiscoverOutcome { plugins, actions })
}

/// Parse a `tumult_validate` `tools/call` result into a [`ValidateOutcome`].
///
/// A failed validation surfaces as `isError: true`; we lift its text into
/// [`McpError::Protocol`] so the loop marks the step failed.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing text.
pub fn parse_validate_result(result: &Value) -> Result<ValidateOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(
            content_text(result).unwrap_or_else(|| "experiment failed validation".to_string()),
        ));
    }
    let text = content_text(result)
        .ok_or_else(|| McpError::Protocol("validate result had no text content".to_string()))?;
    let trimmed = text.trim();
    let valid = trimmed.starts_with("Valid");
    let title = trimmed
        .split_once('\'')
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(t, _)| t.to_string());
    let method_steps = number_before(trimmed, "method step").unwrap_or(0);
    let rollbacks = number_before(trimmed, "rollback").unwrap_or(0);
    Ok(ValidateOutcome {
        valid,
        title,
        method_steps,
        rollbacks,
        summary: trimmed.to_string(),
    })
}

/// Parse a `tumult_analyze_store` `tools/call` result into a [`TableOutcome`].
///
/// The tool returns tab-separated text: a header row, one row per record, then
/// a trailing `N row(s)` line.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error or missing text.
pub fn parse_analyze_store_result(result: &Value) -> Result<TableOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(
            content_text(result).unwrap_or_else(|| "analyze_store tool reported an error".to_string()),
        ));
    }
    let text = content_text(result)
        .ok_or_else(|| McpError::Protocol("analyze_store result had no text content".to_string()))?;

    let mut lines = text.lines();
    let columns: Vec<String> = lines
        .next()
        .map(|h| h.split('\t').map(str::to_string).collect())
        .unwrap_or_default();
    let mut rows = Vec::new();
    for line in lines {
        // The trailing "N row(s)" summary line is not a data row.
        if line.trim_end().ends_with("row(s)") {
            continue;
        }
        if line.is_empty() {
            continue;
        }
        rows.push(line.split('\t').map(str::to_string).collect::<Vec<_>>());
    }
    let row_count = rows.len();
    Ok(TableOutcome {
        columns,
        rows,
        row_count,
    })
}

/// Parse a `tumult_recommend` `tools/call` result into a [`RecommendOutcome`].
///
/// Reads `structuredContent`: either a `message` (no store yet) or a
/// `recommendations` array of `{rank, title, rationale}`. Falls back to the
/// text content when no structured content is present.
///
/// # Errors
/// Returns [`McpError::Protocol`] on a tool-level error.
pub fn parse_recommend_result(result: &Value) -> Result<RecommendOutcome, McpError> {
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(McpError::Protocol(
            content_text(result).unwrap_or_else(|| "recommend tool reported an error".to_string()),
        ));
    }
    if let Some(sc) = result.get("structuredContent") {
        if let Some(msg) = sc.get("message").and_then(Value::as_str) {
            return Ok(RecommendOutcome {
                message: Some(msg.to_string()),
                recommendations: Vec::new(),
            });
        }
        let recommendations = sc
            .get("recommendations")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .map(|r| Recommendation {
                        rank: r.get("rank").and_then(Value::as_i64).unwrap_or_default(),
                        title: r
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        rationale: r
                            .get("rationale")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Ok(RecommendOutcome {
            message: None,
            recommendations,
        });
    }
    // No structured content — fall back to the raw text summary.
    Ok(RecommendOutcome {
        message: content_text(result),
        recommendations: Vec::new(),
    })
}

/// Find the first line beginning with `label` and parse the remainder as a
/// count (e.g. `Plugins: 12` with label `Plugins:` → `12`).
fn labeled_count(text: &str, label: &str) -> Option<usize> {
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix(label)
            .and_then(|rest| rest.trim().parse::<usize>().ok())
    })
}

/// Parse the whitespace-delimited number immediately preceding `suffix`
/// (e.g. `… 3 method steps …` with suffix `method step` → `3`).
fn number_before(text: &str, suffix: &str) -> Option<usize> {
    let idx = text.find(suffix)?;
    text[..idx]
        .split_whitespace()
        .next_back()
        .and_then(|tok| tok.parse::<usize>().ok())
}

/// First text block from a `content` array, if any.
fn content_text(result: &Value) -> Option<String> {
    result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|c| c.iter().find_map(|b| b.get("text").and_then(Value::as_str)))
        .map(ToString::to_string)
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
        let session = session.ok_or_else(|| {
            McpError::Transport("server did not return an mcp-session-id".into())
        })?;
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
        let params = tools_call_params(name, arguments);
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
}

/// The five MCP calls the chaos-loop showcase drives, abstracted so the
/// orchestration can be unit-tested against a mock client. Every method is one
/// `tools/call` over MCP — exactly what an autonomous agent would issue.
pub trait ChaosLoopClient {
    /// `tumult_discover`.
    fn discover(&self)
        -> impl std::future::Future<Output = Result<DiscoverOutcome, McpError>> + Send;
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
    fn recommend(&self)
        -> impl std::future::Future<Output = Result<RecommendOutcome, McpError>> + Send;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_jsonrpc_request_envelope() {
        let r = build_rpc_request(7, "tools/list", json!({"a": 1}));
        assert_eq!(r["jsonrpc"], "2.0");
        assert_eq!(r["id"], 7);
        assert_eq!(r["method"], "tools/list");
        assert_eq!(r["params"]["a"], 1);
    }

    #[test]
    fn notification_has_no_id() {
        let n = build_notification("notifications/initialized", json!({}));
        assert_eq!(n["jsonrpc"], "2.0");
        assert!(n.get("id").is_none());
        assert_eq!(n["method"], "notifications/initialized");
    }

    #[test]
    fn tools_call_params_shape() {
        let p = tools_call_params("tumult_run_experiment", json!({"experiment_path": "x.toon"}));
        assert_eq!(p["name"], "tumult_run_experiment");
        assert_eq!(p["arguments"]["experiment_path"], "x.toon");
    }

    #[test]
    fn parses_sse_framed_payload() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"ok\":true}}\n\n";
        let v = parse_sse_body(body).unwrap();
        assert_eq!(v["result"]["ok"], true);
    }

    #[test]
    fn parses_multiline_sse_data() {
        let body = "data: {\"jsonrpc\":\"2.0\",\ndata: \"id\":1,\"result\":{\"n\":5}}\n\n";
        let v = parse_sse_body(body).unwrap();
        assert_eq!(v["result"]["n"], 5);
    }

    #[test]
    fn parses_plain_json_body() {
        let body = "{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"x\":9}}";
        let v = parse_sse_body(body).unwrap();
        assert_eq!(v["result"]["x"], 9);
    }

    #[test]
    fn empty_body_is_transport_error() {
        assert!(matches!(parse_sse_body(""), Err(McpError::Transport(_))));
    }

    #[test]
    fn rpc_error_maps_to_rpc_variant() {
        let env = json!({"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"bad"}});
        let err = rpc_result(&env).unwrap_err();
        match err {
            McpError::Rpc(m) => assert!(m.contains("bad") && m.contains("-32600")),
            other => panic!("expected Rpc, got {other:?}"),
        }
    }

    #[test]
    fn rpc_result_extracts_result() {
        let env = json!({"jsonrpc":"2.0","id":1,"result":{"tools":[]}});
        assert!(rpc_result(&env).unwrap().get("tools").is_some());
    }

    #[test]
    fn reads_tool_annotations() {
        let result = json!({
            "tools": [
                {
                    "name": "tumult_run_experiment",
                    "description": "Execute a Tumult chaos experiment.",
                    "annotations": { "destructiveHint": true, "readOnlyHint": false }
                },
                {
                    "name": "tumult_validate",
                    "description": "Validate an experiment file.",
                    "annotations": { "destructiveHint": false, "readOnlyHint": true }
                },
                { "name": "tumult_no_annotations", "description": "n/a" }
            ]
        });
        let tools = parse_tools_list(&result);
        assert_eq!(tools.len(), 3);
        let run = tools.iter().find(|t| t.name == "tumult_run_experiment").unwrap();
        assert!(run.destructive);
        assert!(!run.read_only);
        let val = tools.iter().find(|t| t.name == "tumult_validate").unwrap();
        assert!(!val.destructive);
        assert!(val.read_only);
        // Missing annotations default to non-destructive / non-read-only.
        let none = tools.iter().find(|t| t.name == "tumult_no_annotations").unwrap();
        assert!(!none.destructive);
        assert!(!none.read_only);
    }

    #[test]
    fn parses_completed_run_as_passed() {
        // Mirrors Wave A/B run_experiment structuredContent shape.
        let result = json!({
            "content": [{"type":"text","text":"status: completed"}],
            "isError": false,
            "structuredContent": {
                "journal": { "status": "completed", "duration_ms": 228 },
                "journal_path": "/demo/journals/demo-net.toon",
                "ingestion": "ingested"
            }
        });
        let out = parse_run_result(&result).unwrap();
        assert_eq!(out.status, "completed");
        assert_eq!(out.outcome, "passed");
        assert_eq!(out.duration_ms, Some(228));
        assert_eq!(out.journal_path.as_deref(), Some("/demo/journals/demo-net.toon"));
        assert_eq!(out.ingestion.as_deref(), Some("ingested"));
    }

    #[test]
    fn deviated_and_failed_verdicts() {
        let deviated = json!({
            "structuredContent": { "journal": { "status": "deviated", "duration_ms": 10 } }
        });
        assert_eq!(parse_run_result(&deviated).unwrap().outcome, "deviated");
        let failed = json!({
            "structuredContent": { "journal": { "status": "aborted", "duration_ms": 3 } }
        });
        assert_eq!(parse_run_result(&failed).unwrap().outcome, "failed");
    }

    #[test]
    fn tool_level_error_is_protocol_error() {
        let result = json!({
            "isError": true,
            "content": [{"type":"text","text":"experiment file not found"}]
        });
        match parse_run_result(&result) {
            Err(McpError::Protocol(m)) => assert!(m.contains("not found")),
            other => panic!("expected Protocol error, got {other:?}"),
        }
    }

    #[test]
    fn missing_structured_content_is_protocol_error() {
        let result = json!({ "content": [] });
        assert!(matches!(parse_run_result(&result), Err(McpError::Protocol(_))));
    }

    // ── Halted status (auto-halt) ─────────────────────────────

    #[test]
    fn halted_status_maps_to_halted_verdict() {
        assert_eq!(verdict_for("halted"), "halted");
        let result = json!({
            "structuredContent": {
                "journal": { "status": "halted", "duration_ms": 42 },
                "journal_path": "/demo/journals/demo-postgres.toon",
                "ingestion": "ingested"
            }
        });
        let out = parse_run_result(&result).unwrap();
        assert_eq!(out.status, "halted");
        assert_eq!(out.outcome, "halted");
        assert_eq!(out.duration_ms, Some(42));
    }

    // ── discover ──────────────────────────────────────────────

    #[test]
    fn parses_discover_counts() {
        let result = json!({
            "isError": false,
            "content": [{
                "type": "text",
                "text": "Plugins: 3\n  a (native)\n  b (script)\n  c (native)\nActions: 5\n  a::x\n  a::y\n  b::z\n  c::p\n  c::q\n"
            }]
        });
        let d = parse_discover_result(&result).unwrap();
        assert_eq!(d.plugins, 3);
        assert_eq!(d.actions, 5);
    }

    #[test]
    fn discover_missing_counts_is_protocol_error() {
        let result = json!({ "content": [{"type":"text","text":"nothing here"}] });
        assert!(matches!(
            parse_discover_result(&result),
            Err(McpError::Protocol(_))
        ));
    }

    // ── validate ──────────────────────────────────────────────

    #[test]
    fn parses_valid_experiment_summary() {
        let result = json!({
            "isError": false,
            "content": [{
                "type": "text",
                "text": "Valid: 'Kill Postgres connections' — 3 method steps, 1 rollbacks"
            }]
        });
        let v = parse_validate_result(&result).unwrap();
        assert!(v.valid);
        assert_eq!(v.title.as_deref(), Some("Kill Postgres connections"));
        assert_eq!(v.method_steps, 3);
        assert_eq!(v.rollbacks, 1);
    }

    #[test]
    fn invalid_experiment_is_protocol_error() {
        let result = json!({
            "isError": true,
            "content": [{"type":"text","text":"validation error: unknown provider 'nope'"}]
        });
        match parse_validate_result(&result) {
            Err(McpError::Protocol(m)) => assert!(m.contains("unknown provider")),
            other => panic!("expected Protocol error, got {other:?}"),
        }
    }

    // ── analyze_store ─────────────────────────────────────────

    #[test]
    fn parses_analyze_store_table() {
        let result = json!({
            "isError": false,
            "content": [{
                "type": "text",
                "text": "title\tstatus\tduration_ms\nKill connections\thalted\t42\nAdd latency\tcompleted\t228\n2 row(s)"
            }]
        });
        let t = parse_analyze_store_result(&result).unwrap();
        assert_eq!(t.columns, vec!["title", "status", "duration_ms"]);
        assert_eq!(t.row_count, 2);
        assert_eq!(t.rows[0], vec!["Kill connections", "halted", "42"]);
        assert_eq!(t.rows[1], vec!["Add latency", "completed", "228"]);
    }

    #[test]
    fn analyze_store_empty_result_has_no_rows() {
        let result = json!({
            "content": [{"type":"text","text":"title\tstatus\n0 row(s)"}]
        });
        let t = parse_analyze_store_result(&result).unwrap();
        assert_eq!(t.columns, vec!["title", "status"]);
        assert_eq!(t.row_count, 0);
        assert!(t.rows.is_empty());
    }

    #[test]
    fn analyze_store_tool_error_is_protocol_error() {
        let result = json!({
            "isError": true,
            "content": [{"type":"text","text":"store not found: /x/analytics.db"}]
        });
        assert!(matches!(
            parse_analyze_store_result(&result),
            Err(McpError::Protocol(_))
        ));
    }

    // ── recommend ─────────────────────────────────────────────

    #[test]
    fn parses_recommendations_from_structured_content() {
        let result = json!({
            "isError": false,
            "structuredContent": {
                "recommendations": [
                    { "rank": 1, "title": "Test Postgres failover", "rationale": "never exercised" },
                    { "rank": 2, "title": "Add network latency", "rationale": "low coverage" }
                ]
            }
        });
        let r = parse_recommend_result(&result).unwrap();
        assert!(r.message.is_none());
        assert_eq!(r.recommendations.len(), 2);
        assert_eq!(r.recommendations[0].rank, 1);
        assert_eq!(r.recommendations[0].title, "Test Postgres failover");
        assert_eq!(r.recommendations[1].rationale, "low coverage");
    }

    #[test]
    fn recommend_message_when_no_store() {
        let result = json!({
            "structuredContent": { "message": "No analytics store found. Run some experiments first." }
        });
        let r = parse_recommend_result(&result).unwrap();
        assert_eq!(
            r.message.as_deref(),
            Some("No analytics store found. Run some experiments first.")
        );
        assert!(r.recommendations.is_empty());
    }

    #[test]
    fn recommend_tool_error_is_protocol_error() {
        let result = json!({
            "isError": true,
            "content": [{"type":"text","text":"intelligence backend unavailable"}]
        });
        assert!(matches!(
            parse_recommend_result(&result),
            Err(McpError::Protocol(_))
        ));
    }
}
