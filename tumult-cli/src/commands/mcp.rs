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
/// A `--token` value is exported as `TUMULT_MCP_TOKEN` before the handler is
/// built, so the server requires that bearer token on every request; without
/// one it runs unauthenticated (localhost/stdio only).
///
/// # Errors
///
/// Returns an error if the transport fails to start.
pub async fn cmd_mcp_serve(
    transport: Transport,
    host: String,
    port: u16,
    health_port: Option<u16>,
    token: Option<String>,
) -> Result<()> {
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
