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
//!
//! The implementation is split by concern:
//!
//! - [`protocol`] — the JSON-RPC/SSE transport helpers and the [`McpError`]
//!   taxonomy.
//! - [`parse`] — response mapping: the outcome data types and the parsers that
//!   project each `tools/call` result into them.
//! - [`client`] — the live HTTP [`McpClient`], its per-endpoint methods, the
//!   [`ScaffoldArgs`] request shape, and the [`ChaosLoopClient`] abstraction.
//!
//! Every public item is re-exported here, so callers keep using `mcp::…` paths.

mod client;
mod parse;
mod protocol;

pub use client::{ChaosLoopClient, McpClient, ScaffoldArgs};
pub use parse::*;
pub use protocol::*;

#[cfg(test)]
mod tests;
