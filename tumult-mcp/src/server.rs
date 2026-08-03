//! Programmatic entry point for running the Tumult MCP server.
//!
//! Both the standalone `tumult-mcp` binary and the `tumult mcp serve`
//! subcommand (in `tumult-cli`) call [`serve`][crate::server::serve], so the two front-ends share a
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
    /// MCP Streamable HTTP transport.
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

fn allowed_http_origins(host: &str, port: u16) -> Vec<String> {
    let mut origins = vec![format!("http://{host}:{port}")];
    if crate::handler::host_is_loopback(host) || matches!(host, "0.0.0.0" | "::" | "[::]") {
        origins.push(format!("http://127.0.0.1:{port}"));
        origins.push(format!("http://localhost:{port}"));
        origins.sort();
        origins.dedup();
    }
    origins
}

/// Pass-through SDK `AuthProvider`: it authenticates nothing itself. Its only
/// job is to make the SDK's HTTP auth middleware read the `Authorization:
/// Bearer` header and stash the token as `AuthInfo` on the session runtime,
/// where the tool handler picks it up (`resolve_authorization`). Real token
/// validation stays in [`crate::handler::McpAuth`] — fail-closed, with
/// constant-time comparison — so a wrong token still gets the same
/// `Unauthorized` tool-level error as before.
///
/// One behavioral note: with this provider configured (i.e. whenever auth is
/// configured), the SDK middleware answers requests that carry *no*
/// `Authorization` header with `401` at the HTTP layer, before the JSON-RPC
/// payload is read. Over HTTP the header is therefore the required channel;
/// `_meta.authorization` remains the channel on stdio and takes precedence
/// when both are present.
struct HeaderCaptureProvider;

#[async_trait::async_trait]
impl rust_mcp_sdk::auth::AuthProvider for HeaderCaptureProvider {
    async fn verify_token(
        &self,
        access_token: String,
    ) -> Result<rust_mcp_sdk::auth::AuthInfo, rust_mcp_sdk::auth::AuthenticationError> {
        Ok(rust_mcp_sdk::auth::AuthInfo {
            token_unique_id: access_token,
            client_id: None,
            user_id: None,
            scopes: None,
            // The SDK middleware insists on a future expiry; it re-verifies
            // on every request, so the horizon only needs to outlive one
            // request. The token itself is validated later by `McpAuth`.
            expires_at: Some(std::time::SystemTime::now() + std::time::Duration::from_hours(1)),
            audience: None,
            extra: None,
        })
    }

    fn auth_endpoints(
        &self,
    ) -> Option<&std::collections::HashMap<String, rust_mcp_sdk::auth::OauthEndpoint>> {
        None
    }

    async fn handle_request(
        &self,
        _request: rust_mcp_sdk::mcp_http::http::Request<&str>,
        _state: Arc<rust_mcp_sdk::mcp_server::McpAppState>,
    ) -> Result<
        rust_mcp_sdk::mcp_http::http::Response<rust_mcp_sdk::mcp_http::GenericBody>,
        rust_mcp_sdk::mcp_server::error::TransportServerError,
    > {
        // No OAuth endpoints are exposed; anything routed here is a 404.
        use rust_mcp_sdk::mcp_http::GenericBodyExt as _;
        Ok(rust_mcp_sdk::mcp_http::GenericBody::create_404_response())
    }

    fn protected_resource_metadata_url(&self) -> Option<&str> {
        None
    }
}

/// Minimal HTTP health check server using raw TCP.
///
/// Responds to any request on the bound port with a `200 OK` JSON body.
/// Intended for Kubernetes liveness/readiness probes and load balancer checks.
/// Connection tasks are bounded by a semaphore so a slow-connection flood
/// cannot exhaust the runtime.
async fn run_health_server(host: &str, port: u16) {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    /// Maximum concurrent health-check connections being serviced.
    const MAX_HEALTH_CONNECTIONS: usize = 32;

    let addr = format!("{host}:{port}");
    let listener = match TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(addr = %addr, error = %e, "failed to bind health server");
            return;
        }
    };
    tracing::info!("Health endpoint listening on http://{addr}/health");

    let body = format!(
        r#"{{"status":"ok","version":"{}"}}"#,
        env!("CARGO_PKG_VERSION")
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_HEALTH_CONNECTIONS));

    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        // Bound the per-connection tasks: when every slot is held, drop the
        // connection immediately rather than queueing unbounded work.
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            drop(stream);
            continue;
        };
        let resp = response.clone();
        tokio::spawn(async move {
            let _permit = permit;
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
                tracing::info!("received SIGINT, shutting down");
            }
            _ = sigterm.recv() => {
                tracing::info!("received SIGTERM, shutting down");
            }
        }
    }

    #[cfg(not(unix))]
    {
        ctrl_c.await.ok();
        tracing::info!("received SIGINT, shutting down");
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
    // Captured before `auth` moves into the handler: the HTTP transport adds
    // the header-capture middleware only when authentication is configured.
    let auth_configured = auth.is_configured();
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
            tracing::info!(
                "Tumult MCP server listening on http://{}:{}/mcp",
                opts.host,
                opts.port
            );
            let allowed_origins = allowed_http_origins(&opts.host, opts.port);
            let server = hyper_server::create_server(
                details,
                handler,
                HyperServerOptions {
                    host: opts.host,
                    port: opts.port,
                    event_store: Some(Arc::new(InMemoryEventStore::default())),
                    task_store: Some(Arc::new(InMemoryTaskStore::new(None))),
                    client_task_store: Some(Arc::new(InMemoryTaskStore::new(None))),
                    // Capture the HTTP Authorization header onto the session
                    // runtime so handlers can authenticate it. Only when auth
                    // is configured: in open (loopback dev) mode no header is
                    // required and the middleware would 401 its absence.
                    auth: if auth_configured {
                        Some(Arc::new(HeaderCaptureProvider))
                    } else {
                        None
                    },
                    // MCP requires Origin validation for Streamable HTTP to
                    // prevent DNS-rebinding attacks. Non-browser clients that
                    // omit Origin remain supported by the SDK middleware.
                    allowed_origins: Some(allowed_origins),
                    dns_rebinding_protection: true,
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
    tracing::info!("telemetry flushed, exiting");
}

#[cfg(test)]
mod tests {
    use super::{
        allowed_http_origins, run_health_server, serve, server_details, HeaderCaptureProvider,
        ServeOptions, Transport,
    };
    use rust_mcp_sdk::auth::AuthProvider as _;

    #[test]
    fn wildcard_http_bind_accepts_local_browser_origins() {
        let origins = allowed_http_origins("0.0.0.0", 3100);
        assert!(origins.contains(&"http://127.0.0.1:3100".to_string()));
        assert!(origins.contains(&"http://localhost:3100".to_string()));
    }

    #[test]
    fn named_http_bind_only_accepts_its_origin() {
        assert_eq!(
            allowed_http_origins("tumult.internal", 3100),
            vec!["http://tumult.internal:3100"]
        );
    }

    #[test]
    fn loopback_http_bind_adds_local_origins_once() {
        let origins = allowed_http_origins("127.0.0.1", 3100);
        assert_eq!(
            origins
                .iter()
                .filter(|o| *o == "http://127.0.0.1:3100")
                .count(),
            1,
            "loopback origin must not be duplicated: {origins:?}"
        );
        assert!(origins.contains(&"http://localhost:3100".to_string()));
    }

    #[test]
    fn serve_options_default_is_loopback_stdio() {
        let opts = ServeOptions::default();
        assert_eq!(opts.transport, Transport::Stdio);
        assert_eq!(opts.host, "127.0.0.1");
        assert_eq!(opts.port, 3100);
        assert_eq!(opts.health_port, None);
    }

    #[test]
    fn server_details_advertises_tools_resources_and_instructions() {
        let details = server_details();
        assert_eq!(details.server_info.name, "tumult-mcp");
        assert_eq!(details.server_info.version, env!("CARGO_PKG_VERSION"));
        assert!(details.capabilities.tools.is_some());
        assert!(details.capabilities.resources.is_some());
        let instructions = details.instructions.expect("instructions must be set");
        assert!(instructions.contains("chaos engineering"), "{instructions}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn header_capture_provider_passes_the_bearer_token_through() {
        let provider = HeaderCaptureProvider;
        let info = provider
            .verify_token("the-token".to_string())
            .await
            .expect("header capture never rejects");
        assert_eq!(info.token_unique_id, "the-token");
        assert!(
            info.expires_at.expect("expiry must be set") > std::time::SystemTime::now(),
            "the SDK middleware requires a future expiry"
        );
        assert!(provider.auth_endpoints().is_none());
        assert!(provider.protected_resource_metadata_url().is_none());
    }

    /// Bind an ephemeral port and release it, so the health server can take it.
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn health_server_answers_ok_json() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let port = free_port();
        tokio::spawn(run_health_server("127.0.0.1", port));

        let mut response = Vec::new();
        for attempt in 0..50 {
            match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                Ok(mut stream) => {
                    stream
                        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\n\r\n")
                        .await
                        .unwrap();
                    stream.read_to_end(&mut response).await.unwrap();
                    break;
                }
                Err(_) if attempt < 49 => {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                Err(e) => panic!("health server never came up: {e}"),
            }
        }

        let text = String::from_utf8(response).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
        assert!(text.contains("\"status\":\"ok\""), "{text}");
        assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn health_server_logs_and_returns_when_the_port_is_taken() {
        // Hold the port so the health server's bind fails: it must log the
        // error and return promptly rather than panic or hang.
        let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = blocker.local_addr().unwrap().port();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            run_health_server("127.0.0.1", port),
        )
        .await
        .expect("a failed bind must return, not hang");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serve_aborts_on_an_unreadable_auth_config() {
        // An explicit but missing auth config path is a hard startup error:
        // the server must fail closed instead of running unauthenticated.
        let _guard = crate::handler::test_support::AUTH_ENV_LOCK.lock().await;
        let missing = std::env::temp_dir().join(format!(
            "tumult-mcp-test-no-such-auth-{}.toml",
            std::process::id()
        ));
        assert!(!missing.exists());
        std::env::set_var("TUMULT_MCP_AUTH_CONFIG", &missing);
        let result = serve(ServeOptions {
            transport: Transport::Stdio,
            ..ServeOptions::default()
        })
        .await;
        std::env::remove_var("TUMULT_MCP_AUTH_CONFIG");
        let err = result.expect_err("an unreadable auth config must abort startup");
        assert!(err.to_string().contains("auth config"), "got: {err}");
    }
}
