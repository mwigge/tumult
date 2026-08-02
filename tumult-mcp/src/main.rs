//! Tumult MCP Server binary: stdio and Streamable HTTP transports.
//!
//! Thin argument-parsing wrapper around [`tumult_mcp::server::serve`], which
//! owns the actual server wiring so `tumult-cli`'s `tumult mcp serve` and this
//! binary behave identically.

use clap::{Parser, ValueEnum};
use rust_mcp_sdk::error::SdkResult;
use tumult_mcp::server::{serve, ServeOptions, Transport};

/// Drop guard that shuts the OpenTelemetry providers down on every exit path
/// from `main` — including `?` early returns — flushing pending spans and
/// metrics. Mirrors the guard in tumult-cli's `main`; `TumultTelemetry::
/// shutdown` takes `&self`, so `Drop` can call it directly.
struct TelemetryShutdown(tumult_otel::telemetry::TumultTelemetry);

impl Drop for TelemetryShutdown {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

/// Transport for the MCP server.
#[derive(ValueEnum, Clone, Copy, Debug)]
enum TransportArg {
    /// Newline-delimited JSON-RPC over stdin/stdout (default)
    Stdio,
    /// MCP Streamable HTTP transport
    // `sse` was accepted by the previous hand-rolled parser; keep it as a
    // hidden alias so existing invocations keep working.
    #[value(alias = "sse")]
    Http,
}

/// Tumult MCP Server: stdio and Streamable HTTP transports.
#[derive(Parser, Debug)]
#[command(name = "tumult-mcp", version)]
struct Cli {
    /// Transport mode
    #[arg(long, default_value_t = TransportArg::Stdio, value_enum)]
    transport: TransportArg,
    /// Bind address for the HTTP transport and health endpoint
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
    /// operator). Overrides --token; sets `TUMULT_MCP_AUTH_CONFIG`
    #[arg(long)]
    auth_config: Option<std::path::PathBuf>,
}

impl From<Cli> for ServeOptions {
    fn from(cli: Cli) -> Self {
        // The auth flags are consumed by `McpAuth::load()` via the
        // environment when the handler is built — the same mechanism
        // `tumult mcp serve` uses. The config file takes priority when
        // both are given.
        if let Some(path) = &cli.auth_config {
            std::env::set_var("TUMULT_MCP_AUTH_CONFIG", path);
        }
        if let Some(token) = &cli.token {
            std::env::set_var("TUMULT_MCP_TOKEN", token);
        }
        Self {
            transport: match cli.transport {
                TransportArg::Stdio => Transport::Stdio,
                TransportArg::Http => Transport::Http,
            },
            host: cli.host,
            port: cli.port,
            health_port: cli.health_port,
        }
    }
}

#[tokio::main]
async fn main() -> SdkResult<()> {
    // Same quiet-by-default logging policy as tumult-cli: without an OTLP
    // endpoint there is nowhere for telemetry to go, so default to `warn`
    // unless the operator set RUST_LOG explicitly.
    if std::env::var_os("OTEL_EXPORTER_OTLP_ENDPOINT").is_none()
        && std::env::var_os("RUST_LOG").is_none()
    {
        std::env::set_var("RUST_LOG", "warn");
    }

    let args = ServeOptions::from(Cli::parse());

    // Initialize OTel (traces, metrics, and logs — `TumultTelemetry::new`
    // installs all providers internally) before serving, so `tumult-mcp` and
    // `tumult mcp serve` behave identically. Without an OTLP endpoint the
    // providers degrade to noop and the server runs exactly as before. The
    // guard shuts the providers down (flushing) on every exit path. The fmt
    // layer always writes to stderr, keeping stdout clean for stdio framing.
    let otel_config = tumult_otel::config::TelemetryConfig::from_env();
    let _telemetry_guard =
        TelemetryShutdown(tumult_otel::telemetry::TumultTelemetry::new(otel_config));

    serve(args).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_defaults_to_loopback_stdio() {
        let cli = Cli::try_parse_from(["tumult-mcp"]).unwrap();
        assert!(matches!(cli.transport, TransportArg::Stdio));
        assert_eq!(cli.host, "127.0.0.1");
        assert_eq!(cli.port, 3100);
        assert_eq!(cli.health_port, None);
        assert_eq!(cli.token, None);
        assert_eq!(cli.auth_config, None);

        let options = ServeOptions::from(cli);
        assert_eq!(options.transport, Transport::Stdio);
        assert_eq!(options.host, "127.0.0.1");
        assert_eq!(options.port, 3100);
        assert_eq!(options.health_port, None);
    }

    #[test]
    fn cli_maps_http_transport_and_keeps_the_sse_alias() {
        for arg in ["http", "sse"] {
            let cli = Cli::try_parse_from([
                "tumult-mcp",
                "--transport",
                arg,
                "--host",
                "0.0.0.0",
                "--port",
                "8080",
                "--health-port",
                "9090",
            ])
            .unwrap_or_else(|e| panic!("--transport {arg} must parse: {e}"));
            assert!(matches!(cli.transport, TransportArg::Http));

            let options = ServeOptions::from(cli);
            assert_eq!(options.transport, Transport::Http);
            assert_eq!(options.host, "0.0.0.0");
            assert_eq!(options.port, 8080);
            assert_eq!(options.health_port, Some(9090));
        }
    }

    #[test]
    fn cli_rejects_an_unknown_transport() {
        let err = Cli::try_parse_from(["tumult-mcp", "--transport", "grpc"])
            .expect_err("an unknown transport must be rejected");
        assert!(err.to_string().contains("grpc"), "got: {err}");
    }

    #[test]
    fn auth_flags_are_exported_through_the_environment() {
        // The auth flags reach `McpAuth::load()` via the environment, exactly
        // like `tumult mcp serve`. The config file takes priority.
        let config = std::env::temp_dir().join(format!(
            "tumult-mcp-cli-test-auth-{}.toml",
            std::process::id()
        ));
        let cli = Cli::try_parse_from([
            "tumult-mcp",
            "--token",
            "cli-test-token",
            "--auth-config",
            config.to_str().unwrap(),
        ])
        .unwrap();
        let options = ServeOptions::from(cli);
        assert_eq!(options.transport, Transport::Stdio);
        assert_eq!(
            std::env::var("TUMULT_MCP_TOKEN").as_deref(),
            Ok("cli-test-token"),
            "--token must be exported for McpAuth::load"
        );
        assert_eq!(
            std::env::var("TUMULT_MCP_AUTH_CONFIG").as_deref(),
            Ok(config.to_str().unwrap()),
            "--auth-config must be exported for McpAuth::load"
        );
        std::env::remove_var("TUMULT_MCP_TOKEN");
        std::env::remove_var("TUMULT_MCP_AUTH_CONFIG");
    }
}
