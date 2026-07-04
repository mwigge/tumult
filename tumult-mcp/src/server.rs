//! Programmatic entry point for running the Tumult MCP server.
//!
//! Both the standalone `tumult-mcp` binary and the `tumult mcp serve`
//! subcommand (in `tumult-cli`) call [`serve`], so the two front-ends share a
//! single server implementation rather than duplicating transport wiring.

use std::sync::Arc;

use rust_mcp_sdk::{
    error::SdkResult,
    event_store::InMemoryEventStore,
    mcp_server::{hyper_server, server_runtime, HyperServerOptions, McpServerOptions},
    schema::{
        Implementation, InitializeResult, ProtocolVersion, ServerCapabilities,
        ServerCapabilitiesResources, ServerCapabilitiesTools,
    },
    task_store::InMemoryTaskStore,
    McpServer, StdioTransport, ToMcpServerHandler, TransportOptions,
};

/// MCP transport mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Newline-delimited JSON-RPC over stdin/stdout (default).
    Stdio,
    /// Streamable HTTP / SSE transport.
    Http,
}

/// Options controlling how the MCP server runs.
#[derive(Debug, Clone)]
pub struct ServeOptions {
    /// Transport mode.
    pub transport: Transport,
    /// Bind address for the HTTP transport and the health endpoint.
    pub host: String,
    /// Port for the HTTP transport.
    pub port: u16,
    /// Port for the `/health` endpoint. Defaults to `port + 1` when `None`.
    pub health_port: Option<u16>,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            transport: Transport::Stdio,
            // Secure by default: loopback only. Widening the bind requires an
            // explicit host AND a configured token (enforced in `serve`).
            host: String::from("127.0.0.1"),
            port: 3100,
            health_port: None,
        }
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
            // Workspace journals/experiments/gamedays as tumult:// resources.
            // No list-changed notifications or subscriptions yet.
            resources: Some(ServerCapabilitiesResources {
                list_changed: None,
                subscribe: None,
            }),
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

/// Run the Tumult MCP server until it exits or a shutdown signal arrives.
///
/// Spawns a background health-check server (always, regardless of transport),
/// then serves MCP requests over the configured transport. Authentication is
/// resolved via [`crate::handler::McpAuth::load`] (TOML auth config file, or the
/// legacy `TUMULT_MCP_TOKEN` env var → `operator`); a malformed config aborts
/// startup rather than running without authentication.
///
/// # Errors
///
/// Returns any error surfaced by the underlying transport's `start()`.
pub async fn serve(opts: ServeOptions) -> SdkResult<()> {
    let details = server_details();

    // Resolve authentication up front. A malformed/unreadable auth config is a
    // hard startup error — fail closed rather than run without authentication.
    let auth = crate::handler::McpAuth::load().map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("MCP auth config error: {e}"),
        )
    })?;

    // Secure by default: never serve HTTP on a network-exposed address without
    // configured authentication (an auth config file OR TUMULT_MCP_TOKEN). The
    // MCP surface can inject faults and kill containers, so an unauthenticated
    // non-loopback bind is refused outright.
    if matches!(opts.transport, Transport::Http)
        && !crate::handler::host_is_loopback(&opts.host)
        && !auth.is_configured()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "refusing to serve MCP over HTTP on non-loopback address {} without \
                 authentication: provide an auth config (--auth-config / \
                 TUMULT_MCP_AUTH_CONFIG) or set TUMULT_MCP_TOKEN to a strong secret, \
                 or bind --host 127.0.0.1 for local-only access",
                opts.host
            ),
        )
        .into());
    }

    let workspace_root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"));
    let handler =
        crate::handler::TumultHandler::with_auth(workspace_root, auth).to_mcp_server_handler();

    // Determine health port: explicit flag, or MCP port + 1.
    let health_port = opts.health_port.unwrap_or(opts.port.saturating_add(1));
    let health_host = opts.host.clone();

    // Spawn health server in background (always available regardless of transport).
    tokio::spawn(async move {
        run_health_server(&health_host, health_port).await;
    });

    match opts.transport {
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
                opts.host, opts.port
            );
            let server = hyper_server::create_server(
                details,
                handler,
                HyperServerOptions {
                    host: opts.host,
                    port: opts.port,
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
