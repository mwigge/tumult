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
    health_port: Option<u16>,
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
    let mut health_port: Option<u16> = None;

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
            "--health-port" => {
                i += 1;
                if i < args.len() {
                    health_port = Some(args[i].parse().unwrap_or_else(|_| {
                        eprintln!("Invalid health port: {}", args[i]);
                        std::process::exit(1);
                    }));
                }
            }
            "--help" | "-h" => {
                eprintln!("tumult-mcp [OPTIONS]");
                eprintln!();
                eprintln!("Options:");
                eprintln!("  --transport <stdio|http>  Transport mode (default: stdio)");
                eprintln!("  --host <addr>             Bind address for HTTP (default: 0.0.0.0)");
                eprintln!("  --port <port>             Port for HTTP (default: 3100)");
                eprintln!(
                    "  --health-port <port>      Port for /health endpoint (default: port+1)"
                );
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
        health_port,
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

/// Minimal HTTP health check server using raw TCP.
///
/// Responds to any request on the bound port with a `200 OK` JSON body.
/// Intended for Kubernetes liveness/readiness probes and load balancer checks.
async fn run_health_server(host: &str, port: u16) {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    let addr = format!("{host}:{port}");
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Failed to bind health server on {addr}: {e}");
            return;
        }
    };
    eprintln!("Health endpoint listening on http://{addr}/health");

    let body = format!(
        r#"{{"status":"ok","version":"{}"}}"#,
        env!("CARGO_PKG_VERSION")
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );

    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        let resp = response.clone();
        tokio::spawn(async move {
            // Read (and discard) the request — we respond the same regardless of path.
            let mut buf = [0u8; 1024];
            let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
            let _ = stream.write_all(resp.as_bytes()).await;
            let _ = stream.shutdown().await;
        });
    }
}

/// Wait for a shutdown signal (SIGINT or SIGTERM).
async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();

    #[cfg(unix)]
    {
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to register SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {
                eprintln!("received SIGINT, shutting down");
            }
            _ = sigterm.recv() => {
                eprintln!("received SIGTERM, shutting down");
            }
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
        eprintln!("received SIGINT, shutting down");
    }
}

#[tokio::main]
async fn main() -> SdkResult<()> {
    let args = parse_args();
    let details = server_details();
    let handler = tumult_mcp::handler::TumultHandler::default().to_mcp_server_handler();

    // Determine health port: explicit flag, or MCP port + 1 for HTTP, or 3101 for stdio.
    let health_port = args.health_port.unwrap_or(args.port.saturating_add(1));
    let health_host = args.host.clone();

    // Spawn health server in background (always available regardless of transport).
    tokio::spawn(async move {
        run_health_server(&health_host, health_port).await;
    });

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
            tokio::select! {
                result = server.start() => {
                    flush_telemetry();
                    result
                }
                () = shutdown_signal() => {
                    flush_telemetry();
                    Ok(())
                }
            }
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
            tokio::select! {
                result = server.start() => {
                    flush_telemetry();
                    result?;
                    Ok(())
                }
                () = shutdown_signal() => {
                    flush_telemetry();
                    Ok(())
                }
            }
        }
    }
}

/// Flush any pending OpenTelemetry spans before process exit.
fn flush_telemetry() {
    // Replace the global tracer provider with a noop to flush pending spans
    // via the old provider's Drop impl. This mirrors tumult-otel's shutdown logic.
    opentelemetry::global::set_tracer_provider(
        opentelemetry::trace::noop::NoopTracerProvider::new(),
    );
    eprintln!("telemetry flushed, exiting");
}
