//! Tumult MCP Server binary — stdio and HTTP/SSE transport.

use std::sync::Arc;

use rust_mcp_sdk::{
    error::SdkResult,
    event_store::InMemoryEventStore,
    mcp_server::{hyper_server, server_runtime, HyperServerOptions, McpServerOptions},
    schema::{
        Implementation, InitializeResult, ProtocolVersion, ServerCapabilities,
        ServerCapabilitiesTools,
    },
    task_store::InMemoryTaskStore,
    McpServer, StdioTransport, ToMcpServerHandler, TransportOptions,
};

/// Parsed CLI arguments for the MCP server.
struct Args {
    transport: Transport,
    host: String,
    port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Stdio,
    Http,
}

fn parse_args() -> Args {
    let mut transport = Transport::Stdio;
    let mut host = String::from("0.0.0.0");
    let mut port: u16 = 3100;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--transport" => {
                i += 1;
                if i < args.len() {
                    transport = match args[i].as_str() {
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
                    host.clone_from(&args[i]);
                }
            }
            "--port" => {
                i += 1;
                if i < args.len() {
                    port = args[i].parse().unwrap_or_else(|_| {
                        eprintln!("Invalid port: {}", args[i]);
                        std::process::exit(1);
                    });
                }
            }
            "--help" | "-h" => {
                eprintln!("tumult-mcp [OPTIONS]");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --transport <stdio|http>  Transport mode (default: stdio)");
                eprintln!("  --host <addr>             Bind address for HTTP (default: 0.0.0.0)");
                eprintln!("  --port <port>             Port for HTTP (default: 3100)");
                std::process::exit(0);
            }
            _ => {}
        }
        i += 1;
    }

    Args {
        transport,
        host,
        port,
    }
}

fn server_details() -> InitializeResult {
    InitializeResult {
        server_info: Implementation {
            name: "tumult-mcp".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: Some("Tumult Chaos Engineering MCP Server".into()),
            description: Some(
                "Run chaos experiments, analyze journals, discover plugins via MCP tools".into(),
            ),
            icons: vec![],
            website_url: Some("https://github.com/mwigge/tumult".into()),
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            ..Default::default()
        },
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        instructions: Some(
            "Tumult is a Rust-native chaos engineering platform. Use these tools to run \
             resilience experiments, query results with SQL, and discover available chaos \
             actions and probes."
                .into(),
        ),
        meta: None,
    }
}

#[tokio::main]
async fn main() -> SdkResult<()> {
    let args = parse_args();
    let details = server_details();
    let handler = tumult_mcp::handler::TumultHandler::default().to_mcp_server_handler();

    match args.transport {
        Transport::Stdio => {
            let transport = StdioTransport::new(TransportOptions::default())?;
            let server = server_runtime::create_server(McpServerOptions {
                transport,
                handler,
                server_details: details,
                task_store: None,
                client_task_store: None,
                message_observer: None,
            });
            server.start().await
        }
        Transport::Http => {
            eprintln!(
                "Tumult MCP server listening on http://{}:{}/mcp",
                args.host, args.port
            );
            let server = hyper_server::create_server(
                details,
                handler,
                HyperServerOptions {
                    host: args.host,
                    port: args.port,
                    event_store: Some(Arc::new(InMemoryEventStore::default())),
                    task_store: Some(Arc::new(InMemoryTaskStore::new(None))),
                    client_task_store: Some(Arc::new(InMemoryTaskStore::new(None))),
                    ..Default::default()
                },
            );
            server.start().await?;
            Ok(())
        }
    }
}
