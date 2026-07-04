//! `mcp serve` subcommand: launch the Tumult MCP server in-process.
//!
//! `tumult-cli` depends on `tumult-mcp` (an acyclic edge — `tumult-mcp` never
//! depends back on the CLI), so the server runs inside the single `tumult`
//! binary via [`tumult_mcp::server::serve`] rather than shelling out to a
//! separate `tumult-mcp` executable that may not be installed alongside it.

use anyhow::{anyhow, Result};

pub use tumult_mcp::server::Transport;
use tumult_mcp::server::{serve, ServeOptions};

/// Start the MCP server over the chosen transport.
///
/// An `--auth-config <path>` value is exported as `TUMULT_MCP_AUTH_CONFIG` and a
/// `--token` value as `TUMULT_MCP_TOKEN` before the handler is built (the config
/// file takes priority when both are given). With a config file, tokens carry
/// per-request roles (`viewer` / `operator`); a bare `--token` maps to a single
/// `operator` token. Without any auth the server runs unauthenticated
/// (localhost/stdio only — a network HTTP bind is refused).
///
/// # Errors
///
/// Returns an error if the auth config is malformed or the transport fails to
/// start.
pub async fn cmd_mcp_serve(
    transport: Transport,
    host: String,
    port: u16,
    health_port: Option<u16>,
    token: Option<String>,
    auth_config: Option<std::path::PathBuf>,
) -> Result<()> {
    if let Some(path) = auth_config {
        std::env::set_var("TUMULT_MCP_AUTH_CONFIG", path);
    }
    if let Some(token) = token {
        std::env::set_var("TUMULT_MCP_TOKEN", token);
    }
    serve(ServeOptions {
        transport,
        host,
        port,
        health_port,
    })
    .await
    .map_err(|e| anyhow!(e.to_string()))
}
