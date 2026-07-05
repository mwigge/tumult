//! Tumult demo control panel.
//!
//! A self-contained axum web app that serves a single control-panel page and
//! acts as an MCP client of `tumult-mcp`. One card per fault domain; each Run
//! button drives `tumult_run_experiment` over MCP and reports the journal
//! status back to the browser. Resilient by design: if the MCP server is
//! unreachable the page still loads and every endpoint returns a clean error
//! instead of panicking.

mod chaos_loop;
mod handlers;
mod mcp;
mod state;

use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};

use handlers::{
    api_analytics, api_catalog, api_chaosgraph, api_compliance, api_guardrail, api_loop, api_run,
    api_scaffold, api_status, api_whoami, healthz, serve_index,
};
use mcp::McpClient;
use state::{AppState, Config};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,demo_control_panel=info".into()),
        )
        .init();

    let cfg = Config::from_env();
    let token = std::env::var("TUMULT_MCP_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    if token.is_none() {
        tracing::warn!(
            "TUMULT_MCP_TOKEN not set — MCP requests will be sent without a bearer token"
        );
    }
    let client = McpClient::new(&cfg.mcp_url, token);

    let index_html = state::render_index(&cfg);
    let port = cfg.port;
    let state = Arc::new(AppState {
        cfg,
        client,
        index_html,
    });

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/healthz", get(healthz))
        .route("/api/status", get(api_status))
        .route("/api/whoami", get(api_whoami))
        .route("/api/run/{domain}", post(api_run))
        .route("/api/loop", post(api_loop))
        .route("/api/guardrail", post(api_guardrail))
        .route("/api/compliance", get(api_compliance))
        .route("/api/analytics", get(api_analytics))
        .route("/api/chaosgraph", get(api_chaosgraph))
        .route("/api/catalog", get(api_catalog))
        .route("/api/scaffold", post(api_scaffold))
        .with_state(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!("demo-control-panel listening on http://{addr}");
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("failed to bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
    {
        tracing::error!("server error: {e}");
    }
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}
