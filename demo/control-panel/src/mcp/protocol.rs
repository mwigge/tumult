//! Transport-agnostic JSON-RPC 2.0 protocol layer: the [`McpError`] taxonomy,
//! request/notification builders, and the Streamable-HTTP (SSE) envelope
//! parsing. Everything here is a pure function unit-tested against canned JSON.

use serde_json::{json, Value};

/// Protocol version we advertise on `initialize`.
const PROTOCOL_VERSION: &str = "2025-11-25";

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
