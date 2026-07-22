//! Tumult MCP Server binary: stdio and Streamable HTTP transports.
//!
//! Thin argument-parsing wrapper around [`tumult_mcp::server::serve`], which
//! owns the actual server wiring so `tumult-cli`'s `tumult mcp serve` and this
//! binary behave identically.

use rust_mcp_sdk::error::SdkResult;
use tumult_mcp::server::{serve, ServeOptions, Transport};

fn parse_args() -> ServeOptions {
    let mut opts = ServeOptions::default();

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--transport" => {
                i += 1;
                if i < args.len() {
                    opts.transport = match args[i].as_str() {
                        "http" | "sse" => Transport::Http,
                        "stdio" => Transport::Stdio,
                        other => {
                            eprintln!("Unknown transport: {other}. Use 'stdio' or 'http'.");
                            std::process::exit(1);
                        }
                    };
                }
            }
            "--host" => {
                i += 1;
                if i < args.len() {
                    opts.host.clone_from(&args[i]);
                }
            }
            "--port" => {
                i += 1;
                if i < args.len() {
                    opts.port = args[i].parse().unwrap_or_else(|_| {
                        eprintln!("Invalid port: {}", args[i]);
                        std::process::exit(1);
                    });
                }
            }
            "--health-port" => {
                i += 1;
                if i < args.len() {
                    opts.health_port = Some(args[i].parse().unwrap_or_else(|_| {
                        eprintln!("Invalid health port: {}", args[i]);
                        std::process::exit(1);
                    }));
                }
            }
            "--auth-config" => {
                i += 1;
                if i < args.len() {
                    // Consumed by McpAuth::load() via the environment.
                    std::env::set_var("TUMULT_MCP_AUTH_CONFIG", &args[i]);
                }
            }
            "--help" | "-h" => {
                eprintln!("tumult-mcp [OPTIONS]");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --transport <stdio|http>  Transport mode (default: stdio)");
                eprintln!("  --host <addr>             Bind address for HTTP (default: 127.0.0.1)");
                eprintln!("  --port <port>             Port for HTTP (default: 3100)");
                eprintln!(
                    "  --health-port <port>      Port for /health endpoint (default: port+1)"
                );
                eprintln!(
                    "  --auth-config <path>      TOML auth config (token->role); sets TUMULT_MCP_AUTH_CONFIG"
                );
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    opts
}

#[tokio::main]
async fn main() -> SdkResult<()> {
    serve(parse_args()).await
}
