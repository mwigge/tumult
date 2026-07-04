//! Tumult demo control panel.
//!
//! A self-contained axum web app that serves a single control-panel page and
//! acts as an MCP client of `tumult-mcp`. One card per fault domain; each Run
//! button drives `tumult_run_experiment` over MCP and reports the journal
//! status back to the browser. Resilient by design: if the MCP server is
//! unreachable the page still loads and every endpoint returns a clean error
//! instead of panicking.

mod mcp;

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::json;

use mcp::{McpClient, McpError};

/// The seven demo fault domains from CONTRACT.md, in display order.
const DOMAINS: &[(&str, &str)] = &[
    ("net", "Injects latency on the network path between demo-app and demo-postgres (tumult-net userspace proxy)."),
    ("postgres", "Kills active Postgres connections mid-flight (tumult-db-postgres script plugin)."),
    ("container", "Pauses the demo-postgres container briefly (tumult-pumba / container pause)."),
    ("stress", "Applies CPU and memory pressure to the demo-app container (tumult-stress)."),
    ("process", "Injects a process fault against demo-app."),
    ("ssh", "Runs a native fault against the demo-sshd target over SSH (tumult-ssh)."),
    ("agentic", "Runs a bundled agentic resilience scenario — no external API (fake adapter)."),
];

/// Runtime configuration, all from environment with demo-friendly defaults.
#[derive(Clone)]
struct Config {
    /// MCP base URL, e.g. `http://tumult-mcp:3100` (path `/mcp` appended by client).
    mcp_url: String,
    /// Directory the experiments are mounted at *inside the tumult-mcp container*.
    experiments_dir: String,
    /// SigNoz base URL for the "View traces" deep link.
    signoz_url: String,
    /// Demo app URL for the top status bar link.
    demo_app_url: String,
    /// Service name to filter traces by in SigNoz.
    trace_service: String,
    /// Bind port.
    port: u16,
}

impl Config {
    fn from_env() -> Self {
        let mcp_url = env_or("MCP_URL", "http://tumult-mcp:3100");
        let experiments_dir = trim_trailing_slash(&env_or("DEMO_EXPERIMENTS_DIR", "/demo/experiments"));
        Self {
            mcp_url,
            experiments_dir,
            signoz_url: trim_trailing_slash(&env_or("SIGNOZ_URL", "http://localhost:3301")),
            demo_app_url: trim_trailing_slash(&env_or("DEMO_APP_URL", "http://localhost:8080")),
            trace_service: env_or("TRACE_SERVICE", "demo-app"),
            port: env_or("PORT", "8088").parse().unwrap_or(8088),
        }
    }

    /// Experiment path for a domain, as seen inside the tumult-mcp container.
    fn experiment_path(&self, domain: &str) -> String {
        format!("{}/demo-{}.toon", self.experiments_dir, domain)
    }

    /// Best-effort SigNoz deep link to the traces explorer, hinting the service
    /// to filter by. SigNoz's exact filter query params vary across versions,
    /// so we link to the traces explorer page; the UI also shows the service
    /// name (`demo-app`) to filter on.
    fn signoz_trace_link(&self) -> String {
        format!(
            "{}/traces-explorer?selected={}",
            self.signoz_url,
            urlencode(&format!("{{\"service.name\":\"{}\"}}", self.trace_service))
        )
    }
}

/// Shared application state.
struct AppState {
    cfg: Config,
    client: McpClient,
    /// Page HTML with server-side config substituted in.
    index_html: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,demo_control_panel=info".into()),
        )
        .init();

    let cfg = Config::from_env();
    let token = std::env::var("TUMULT_MCP_TOKEN").ok().filter(|t| !t.is_empty());
    if token.is_none() {
        tracing::warn!("TUMULT_MCP_TOKEN not set — MCP requests will be sent without a bearer token");
    }
    let client = McpClient::new(&cfg.mcp_url, token);

    let index_html = render_index(&cfg);
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
        .route("/api/run/{domain}", post(api_run))
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

// ── Handlers ──────────────────────────────────────────────────

async fn serve_index(State(state): State<Arc<AppState>>) -> Html<String> {
    Html(state.index_html.clone())
}

async fn healthz() -> impl IntoResponse {
    (StatusCode::OK, Json(json!({ "status": "ok" })))
}

#[derive(Serialize)]
struct DomainView {
    id: String,
    description: String,
    experiment_path: String,
    /// True when the tumult_run_experiment tool advertised destructiveHint.
    destructive: bool,
}

#[derive(Serialize)]
struct StatusView {
    mcp_online: bool,
    mcp_error: Option<String>,
    tool_count: usize,
    run_tool_destructive: bool,
    signoz_url: String,
    signoz_trace_link: String,
    demo_app_url: String,
    trace_service: String,
    domains: Vec<DomainView>,
}

/// Top-status-bar + per-domain state. Calls `tools/list` to check reachability
/// and read the run-experiment annotation. Always returns 200 with a body — an
/// offline MCP server is reported in-band so the page degrades gracefully.
async fn api_status(State(state): State<Arc<AppState>>) -> Json<StatusView> {
    let (mcp_online, mcp_error, tool_count, run_destructive) =
        match state.client.list_tools().await {
            Ok(tools) => {
                let destructive = tools
                    .iter()
                    .find(|t| t.name == "tumult_run_experiment")
                    .map(|t| t.destructive)
                    // If the run tool is not advertised, be conservative.
                    .unwrap_or(true);
                (true, None, tools.len(), destructive)
            }
            Err(e) => {
                tracing::warn!("tools/list failed: {e}");
                // Conservative default: treat runs as destructive when unknown.
                (false, Some(e.to_string()), 0, true)
            }
        };

    let cfg = &state.cfg;
    let domains = DOMAINS
        .iter()
        .map(|(id, desc)| DomainView {
            id: (*id).to_string(),
            description: (*desc).to_string(),
            experiment_path: cfg.experiment_path(id),
            destructive: run_destructive,
        })
        .collect();

    Json(StatusView {
        mcp_online,
        mcp_error,
        tool_count,
        run_tool_destructive: run_destructive,
        signoz_url: cfg.signoz_url.clone(),
        signoz_trace_link: cfg.signoz_trace_link(),
        demo_app_url: cfg.demo_app_url.clone(),
        trace_service: cfg.trace_service.clone(),
        domains,
    })
}

#[derive(Serialize)]
struct RunResponse {
    domain: String,
    status: String,
    outcome: String,
    duration_ms: Option<u64>,
    journal_path: Option<String>,
    ingestion: Option<String>,
    experiment_path: String,
    signoz_trace_link: String,
}

/// Run one domain's experiment via MCP. Unknown domains → 404; MCP failures →
/// a clean JSON error with an appropriate status code (never a panic).
async fn api_run(
    State(state): State<Arc<AppState>>,
    Path(domain): Path<String>,
) -> Response {
    if !DOMAINS.iter().any(|(id, _)| *id == domain) {
        return error_response(
            StatusCode::NOT_FOUND,
            &format!("unknown fault domain '{domain}'"),
        );
    }

    let path = state.cfg.experiment_path(&domain);
    match state.client.run_experiment(&path).await {
        Ok(out) => Json(RunResponse {
            domain,
            status: out.status,
            outcome: out.outcome,
            duration_ms: out.duration_ms,
            journal_path: out.journal_path,
            ingestion: out.ingestion,
            experiment_path: path,
            signoz_trace_link: state.cfg.signoz_trace_link(),
        })
        .into_response(),
        Err(e) => {
            tracing::warn!("run {domain} failed: {e}");
            let code = match e {
                McpError::Unreachable(_) => StatusCode::SERVICE_UNAVAILABLE,
                McpError::Rpc(_) | McpError::Protocol(_) => StatusCode::BAD_GATEWAY,
                McpError::Transport(_) => StatusCode::BAD_GATEWAY,
            };
            error_response(code, &e.to_string())
        }
    }
}

fn error_response(code: StatusCode, message: &str) -> Response {
    (code, Json(json!({ "error": message }))).into_response()
}

// ── Helpers ───────────────────────────────────────────────────

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn trim_trailing_slash(s: &str) -> String {
    s.trim_end_matches('/').to_string()
}

/// Minimal percent-encoding for the handful of characters we embed in the
/// SigNoz query string. Avoids pulling in a URL-encoding crate.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Inject the runtime config into the static HTML as a JSON blob the page's JS
/// reads on load, then hand back the full document.
fn render_index(cfg: &Config) -> String {
    let bootstrap = json!({
        "signozUrl": cfg.signoz_url,
        "signozTraceLink": cfg.signoz_trace_link(),
        "demoAppUrl": cfg.demo_app_url,
        "traceService": cfg.trace_service,
    })
    .to_string();
    include_str!("../static/index.html").replace("/*__CONFIG__*/null", &bootstrap)
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
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
