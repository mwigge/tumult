//! `mcp` subcommand: transport enum and action arguments.

/// Transport for `tumult mcp serve`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpTransport {
    /// JSON-RPC over stdin/stdout (default)
    Stdio,
    /// Streamable HTTP / SSE
    Http,
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum McpAction {
    /// Start the MCP server (stdio for local agents, HTTP for networked use)
    Serve {
        /// Transport mode
        #[arg(long, default_value_t = McpTransport::Stdio, value_enum)]
        transport: McpTransport,
        /// Bind address for the HTTP transport and health endpoint. Loopback by
        /// default; a non-loopback bind (e.g. 0.0.0.0) requires --token.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port for the HTTP transport
        #[arg(long, default_value_t = 3100)]
        port: u16,
        /// Port for the /health endpoint (default: port + 1)
        #[arg(long)]
        health_port: Option<u16>,
        /// Require this bearer token on every request (sets `TUMULT_MCP_TOKEN`,
        /// mapped to the `operator` role)
        #[arg(long)]
        token: Option<String>,
        /// Path to a TOML auth config file granting per-token roles (viewer /
        /// operator). Overrides --token; sets `TUMULT_MCP_AUTH_CONFIG`.
        #[arg(long)]
        auth_config: Option<std::path::PathBuf>,
    },
}
