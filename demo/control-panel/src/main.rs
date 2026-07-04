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
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use mcp::{ChaosLoopClient, McpClient, McpError};

/// SQL the chaos-loop's analyze step runs over the persistent analytics store:
/// the five most recent experiments with their status and duration.
const LOOP_ANALYZE_SQL: &str =
    "SELECT title, status, duration_ms FROM experiments ORDER BY started_at_ns DESC LIMIT 5";

/// SQL the Analytics card runs over the persistent store: the most recent
/// experiments with title, status and duration.
const ANALYTICS_SQL: &str =
    "SELECT title, status, duration_ms FROM experiments ORDER BY started_at_ns DESC LIMIT 8";

/// Default fault domain the chaos loop validates and runs when none is given.
const DEFAULT_LOOP_DOMAIN: &str = "postgres";

/// The auto-halt guardrail experiment. Kept out of the pass/fail sweep: its
/// expected outcome is `Halted` (the guard pulls the run mid-flight), not
/// `Completed`, so it has its own "Safety guardrail" card.
const GUARD_HALT_DOMAIN: &str = "guard-halt";

/// The demo fault domains from CONTRACT.md (plus the two timewarp domains), in
/// display order. Each runs `demo-<id>.toon` and completes on success, so they
/// double as the fault sweep and the per-domain cards.
const DOMAINS: &[(&str, &str)] = &[
    ("net", "Injects latency on the network path between demo-app and demo-postgres (tumult-net userspace proxy)."),
    ("postgres", "Kills active Postgres connections mid-flight (tumult-db-postgres script plugin)."),
    ("container", "Pauses the demo-postgres container briefly (tumult-pumba / container pause)."),
    ("stress", "Applies CPU and memory pressure to the demo-app container (tumult-stress)."),
    ("process", "Injects a process fault against demo-app."),
    ("ssh", "Runs a native fault against the demo-sshd target over SSH (tumult-ssh)."),
    ("agentic", "Runs a bundled agentic resilience scenario — no external API (fake adapter)."),
    ("timewarp-clock", "Advances a validator's perceived clock past a short-TTL token's expiry and proves the once-valid token is rejected, while demo-app stays healthy (tumult-timewarp)."),
    ("timewarp-entropy", "Applies sustained RNG/crypto pressure on the runner and proves crypto still completes and entropy stays readable (tumult-timewarp)."),
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
    /// Directory (inside the tumult-mcp container) the fault sweep writes its
    /// journals into — the corpus the Compliance card evaluates.
    journals_dir: String,
    /// Regulatory framework the Compliance / ChaosGraph cards report against.
    compliance_framework: String,
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
            journals_dir: trim_trailing_slash(&env_or("DEMO_JOURNALS_DIR", "/journals")),
            compliance_framework: env_or("DEMO_COMPLIANCE_FRAMEWORK", "dora"),
            port: env_or("PORT", "8088").parse().unwrap_or(8088),
        }
    }

    /// Experiment path for a domain, as seen inside the tumult-mcp container.
    fn experiment_path(&self, domain: &str) -> String {
        format!("{}/demo-{}.toon", self.experiments_dir, domain)
    }

    /// SigNoz deep link to the traces explorer. SigNoz's exact pre-filter query
    /// params vary across versions, so we link to the traces-explorer page
    /// (which always exists on the standalone build) and the UI states the
    /// service to filter on (`demo-app`) explicitly — a working link and a
    /// clear filter instruction rather than a fragile pre-filter that may 404.
    fn signoz_trace_link(&self) -> String {
        format!("{}/traces-explorer", self.signoz_url)
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
        .route("/api/loop", post(api_loop))
        .route("/api/guardrail", post(api_guardrail))
        .route("/api/compliance", get(api_compliance))
        .route("/api/analytics", get(api_analytics))
        .route("/api/chaosgraph", get(api_chaosgraph))
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
    /// The auto-halt guardrail experiment (own card; expected outcome Halted).
    guard_halt: DomainView,
    /// Directory the sweep's journals live in (Compliance card corpus).
    journals_dir: String,
    /// Framework the Compliance / ChaosGraph cards report against.
    compliance_framework: String,
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
        guard_halt: DomainView {
            id: GUARD_HALT_DOMAIN.to_string(),
            description:
                "A safety guard watches demo-app health during a DB outage and halts the run the \
                 moment it turns unhealthy, running rollback immediately. Expected outcome: Halted."
                    .to_string(),
            experiment_path: cfg.experiment_path(GUARD_HALT_DOMAIN),
            destructive: run_destructive,
        },
        journals_dir: cfg.journals_dir.clone(),
        compliance_framework: cfg.compliance_framework.clone(),
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

/// Map an [`McpError`] to the HTTP status the panel reports it under.
fn status_for(err: &McpError) -> StatusCode {
    match err {
        McpError::Unreachable(_) => StatusCode::SERVICE_UNAVAILABLE,
        McpError::Rpc(_) | McpError::Protocol(_) | McpError::Transport(_) => StatusCode::BAD_GATEWAY,
    }
}

// ── Golden-path payoff cards ──────────────────────────────────
// Compliance, Analytics, and ChaosGraph each drive one (or a few) EXISTING
// read-only MCP tools, surfacing the enterprise payoff a viewer would
// otherwise only see in scripts/gameday-demo.sh.

/// Run the auto-halt guardrail experiment (`demo-guard-halt.toon`) via MCP.
/// Expected outcome is `Halted` — the guard pulls the run when demo-app turns
/// unhealthy and rollback restores the database. MCP failures → a clean JSON
/// error, never a panic.
async fn api_guardrail(State(state): State<Arc<AppState>>) -> Response {
    let path = state.cfg.experiment_path(GUARD_HALT_DOMAIN);
    match state.client.run_experiment(&path).await {
        Ok(out) => Json(RunResponse {
            domain: GUARD_HALT_DOMAIN.to_string(),
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
            tracing::warn!("guardrail run failed: {e}");
            error_response(status_for(&e), &e.to_string())
        }
    }
}

/// Compliance card: `tumult_compliance {framework, journals_path}` over the
/// sweep's journal corpus. Returns the evidence pass rate / verdict / citations
/// and the tool's own scope disclaimer.
async fn api_compliance(State(state): State<Arc<AppState>>) -> Response {
    let cfg = &state.cfg;
    match state
        .client
        .compliance(&cfg.compliance_framework, &cfg.journals_dir)
        .await
    {
        Ok(out) => Json(json!({
            "journals_path": cfg.journals_dir,
            "compliance": out,
        }))
        .into_response(),
        Err(e) => {
            tracing::warn!("compliance failed: {e}");
            error_response(status_for(&e), &e.to_string())
        }
    }
}

/// Analytics card: `tumult_analyze_store` over the persistent store — the most
/// recent experiments (title / status / duration) as a compact table.
async fn api_analytics(State(state): State<Arc<AppState>>) -> Response {
    match state.client.analyze_store(ANALYTICS_SQL).await {
        Ok(table) => Json(json!({
            "sql": ANALYTICS_SQL,
            "columns": table.columns,
            "rows": table.rows,
            "row_count": table.row_count,
        }))
        .into_response(),
        Err(e) => {
            tracing::warn!("analytics failed: {e}");
            error_response(status_for(&e), &e.to_string())
        }
    }
}

/// One section of the ChaosGraph card: its data, or the reason it was empty.
/// Sections degrade independently so a partial graph still renders.
#[derive(Serialize)]
struct GraphSection<T: Serialize> {
    data: Option<T>,
    error: Option<String>,
}

impl<T: Serialize> GraphSection<T> {
    fn from(result: Result<T, McpError>) -> Self {
        match result {
            Ok(data) => Self { data: Some(data), error: None },
            Err(e) => Self { data: None, error: Some(e.to_string()) },
        }
    }
}

#[derive(Serialize)]
struct ChaosGraphView {
    framework: String,
    /// Fault nodes recorded in the store (`tumult_chaosgraph_query kind=fault`).
    faults: GraphSection<mcp::GraphNodesOutcome>,
    /// The centre experiment the ego sub-graph is drawn around, if one exists.
    ego_center: Option<mcp::GraphNode>,
    /// The chosen experiment's ego sub-graph (`tumult_chaosgraph_neighbors`).
    ego: GraphSection<mcp::GraphEgoOutcome>,
    /// Untested actions + unevidenced articles (`..._coverage_gaps`).
    coverage_gaps: GraphSection<mcp::CoverageGapsOutcome>,
}

/// ChaosGraph card: makes the flagship graph visible to a human. Drives three
/// EXISTING read-only tools — `tumult_chaosgraph_query` (fault nodes), then
/// `..._neighbors` on a chosen experiment node (its sub-graph), and
/// `..._coverage_gaps` (untested actions + unevidenced framework articles).
/// Always 200: each section reports its own error in-band so an empty store or
/// a mid-call failure still renders the rest of the graph.
async fn api_chaosgraph(State(state): State<Arc<AppState>>) -> Response {
    let client = &state.client;
    let framework = state.cfg.compliance_framework.clone();

    // 1 · Fault nodes.
    let faults = client.chaosgraph_query("fault", None).await;

    // 2 · Pick an experiment node to centre the ego sub-graph on, then fetch it.
    let (ego_center, ego) = match client.chaosgraph_query("experiment", None).await {
        Ok(exps) => match exps.nodes.into_iter().next() {
            Some(center) => {
                let ego = client.chaosgraph_neighbors(&center.id, None, 1).await;
                (Some(center), GraphSection::from(ego))
            }
            None => (
                None,
                GraphSection {
                    data: None,
                    error: Some("no experiment nodes in the store yet".to_string()),
                },
            ),
        },
        Err(e) => (None, GraphSection { data: None, error: Some(e.to_string()) }),
    };

    // 3 · Coverage gaps for the framework (untested actions + unevidenced articles).
    let coverage_gaps = client
        .chaosgraph_coverage_gaps(Some(&framework), None)
        .await;

    Json(ChaosGraphView {
        framework,
        faults: GraphSection::from(faults),
        ego_center,
        ego,
        coverage_gaps: GraphSection::from(coverage_gaps),
    })
    .into_response()
}

// ── Chaos loop showcase ───────────────────────────────────────

/// One step of the full discover→validate→run→analyze→recommend loop, as
/// rendered on the UI timeline. Each step is exactly one MCP `tools/call`.
#[derive(Serialize, Clone)]
struct LoopStep {
    /// 1-based position in the sequence.
    index: usize,
    /// Human step name, e.g. "Discover".
    name: String,
    /// The MCP tool this step invoked, e.g. "tumult_discover".
    tool: String,
    /// "ok" | "error".
    status: String,
    /// Wall-clock time this single MCP call took.
    elapsed_ms: u64,
    /// One-line result summary for the timeline.
    summary: String,
    /// Structured payload for the step (counts, table rows, recommendations…).
    detail: serde_json::Value,
    /// Present only when `status == "error"`.
    error: Option<String>,
}

impl LoopStep {
    fn ok(
        index: usize,
        name: &str,
        tool: &str,
        elapsed_ms: u64,
        summary: String,
        detail: serde_json::Value,
    ) -> Self {
        Self {
            index,
            name: name.to_string(),
            tool: tool.to_string(),
            status: "ok".to_string(),
            elapsed_ms,
            summary,
            detail,
            error: None,
        }
    }

    fn failed(index: usize, name: &str, tool: &str, elapsed_ms: u64, err: &McpError) -> Self {
        Self {
            index,
            name: name.to_string(),
            tool: tool.to_string(),
            status: "error".to_string(),
            elapsed_ms,
            summary: err.to_string(),
            detail: json!({}),
            error: Some(err.to_string()),
        }
    }
}

/// Full result of one chaos-loop run.
#[derive(Serialize)]
struct LoopReport {
    /// True only when all five steps completed successfully.
    ok: bool,
    experiment_path: String,
    steps: Vec<LoopStep>,
    /// Deep link to SigNoz traces for the run step.
    signoz_trace_link: String,
}

/// Drive discover → validate → run → analyze → recommend as five separate MCP
/// tool calls, stopping at the first failure. Never panics; a failing step is
/// recorded and the loop returns early with `ok == false`. Generic over the
/// client so the orchestration is unit-tested against a mock.
async fn run_chaos_loop<C: ChaosLoopClient>(
    client: &C,
    experiment_path: &str,
) -> (bool, Vec<LoopStep>) {
    use std::time::Instant;
    let mut steps = Vec::with_capacity(5);

    // 1 · Discover
    let t = Instant::now();
    match client.discover().await {
        Ok(d) => steps.push(LoopStep::ok(
            1,
            "Discover",
            "tumult_discover",
            elapsed_ms(t),
            format!("{} plugins · {} actions available", d.plugins, d.actions),
            json!({ "plugins": d.plugins, "actions": d.actions }),
        )),
        Err(e) => {
            steps.push(LoopStep::failed(1, "Discover", "tumult_discover", elapsed_ms(t), &e));
            return (false, steps);
        }
    }

    // 2 · Validate
    let t = Instant::now();
    match client.validate(experiment_path).await {
        Ok(v) => steps.push(LoopStep::ok(
            2,
            "Validate",
            "tumult_validate",
            elapsed_ms(t),
            format!(
                "{} · {} method step{}",
                if v.valid { "valid" } else { "invalid" },
                v.method_steps,
                if v.method_steps == 1 { "" } else { "s" }
            ),
            json!({
                "valid": v.valid,
                "title": v.title,
                "method_steps": v.method_steps,
                "rollbacks": v.rollbacks,
            }),
        )),
        Err(e) => {
            steps.push(LoopStep::failed(2, "Validate", "tumult_validate", elapsed_ms(t), &e));
            return (false, steps);
        }
    }

    // 3 · Run
    let t = Instant::now();
    match client.run_experiment(experiment_path).await {
        Ok(r) => {
            let dur = r
                .duration_ms
                .map_or_else(|| "—".to_string(), |d| format!("{d} ms"));
            steps.push(LoopStep::ok(
                3,
                "Run",
                "tumult_run_experiment",
                elapsed_ms(t),
                format!("{} · {}", r.status, dur),
                json!({
                    "status": r.status,
                    "outcome": r.outcome,
                    "duration_ms": r.duration_ms,
                    "journal_path": r.journal_path,
                    "ingestion": r.ingestion,
                }),
            ));
        }
        Err(e) => {
            steps.push(LoopStep::failed(3, "Run", "tumult_run_experiment", elapsed_ms(t), &e));
            return (false, steps);
        }
    }

    // 4 · Analyze
    let t = Instant::now();
    match client.analyze_store(LOOP_ANALYZE_SQL).await {
        Ok(table) => steps.push(LoopStep::ok(
            4,
            "Analyze",
            "tumult_analyze_store",
            elapsed_ms(t),
            format!(
                "{} recent experiment{}",
                table.row_count,
                if table.row_count == 1 { "" } else { "s" }
            ),
            json!({
                "sql": LOOP_ANALYZE_SQL,
                "columns": table.columns,
                "rows": table.rows,
                "row_count": table.row_count,
            }),
        )),
        Err(e) => {
            steps.push(LoopStep::failed(4, "Analyze", "tumult_analyze_store", elapsed_ms(t), &e));
            return (false, steps);
        }
    }

    // 5 · Recommend
    let t = Instant::now();
    match client.recommend().await {
        Ok(rec) => {
            let summary = if let Some(msg) = &rec.message {
                msg.clone()
            } else if let Some(top) = rec.recommendations.first() {
                format!(
                    "{} recommendation{} · top: {}",
                    rec.recommendations.len(),
                    if rec.recommendations.len() == 1 { "" } else { "s" },
                    top.title
                )
            } else {
                "no recommendations".to_string()
            };
            steps.push(LoopStep::ok(
                5,
                "Recommend",
                "tumult_recommend",
                elapsed_ms(t),
                summary,
                json!({
                    "message": rec.message,
                    "recommendations": rec.recommendations,
                }),
            ));
        }
        Err(e) => {
            steps.push(LoopStep::failed(5, "Recommend", "tumult_recommend", elapsed_ms(t), &e));
            return (false, steps);
        }
    }

    (true, steps)
}

fn elapsed_ms(t: std::time::Instant) -> u64 {
    // Saturating cast is fine: step durations never approach u64::MAX ms.
    u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Query params for the loop endpoint: an optional fault `domain` selecting the
/// experiment to validate and run (defaults to `postgres`).
#[derive(Deserialize, Default)]
struct LoopParams {
    domain: Option<String>,
}

/// Run the whole chaos loop via MCP and return every step's result. Always
/// returns 200 with a body — an offline MCP server or a mid-loop failure is
/// reported in-band as a failed step, never a panic. Unknown domain → 404.
async fn api_loop(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LoopParams>,
) -> Response {
    let domain = params
        .domain
        .unwrap_or_else(|| DEFAULT_LOOP_DOMAIN.to_string());
    if !DOMAINS.iter().any(|(id, _)| *id == domain) {
        return error_response(
            StatusCode::NOT_FOUND,
            &format!("unknown fault domain '{domain}'"),
        );
    }
    let path = state.cfg.experiment_path(&domain);
    let (ok, steps) = run_chaos_loop(&state.client, &path).await;
    if !ok {
        tracing::warn!("chaos loop for '{domain}' halted at step {}", steps.len());
    }
    Json(LoopReport {
        ok,
        experiment_path: path,
        steps,
        signoz_trace_link: state.cfg.signoz_trace_link(),
    })
    .into_response()
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

/// Inject the runtime config into the static HTML as a JSON blob the page's JS
/// reads on load, then hand back the full document.
fn render_index(cfg: &Config) -> String {
    let bootstrap = json!({
        "signozUrl": cfg.signoz_url,
        "signozTraceLink": cfg.signoz_trace_link(),
        "demoAppUrl": cfg.demo_app_url,
        "traceService": cfg.trace_service,
        "journalsDir": cfg.journals_dir,
        "complianceFramework": cfg.compliance_framework,
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

#[cfg(test)]
mod tests {
    use super::*;
    use mcp::{
        ChaosLoopClient, DiscoverOutcome, McpError, Recommendation, RecommendOutcome, RunOutcome,
        TableOutcome, ValidateOutcome,
    };

    /// A canned [`ChaosLoopClient`] for testing the orchestration without a
    /// live MCP server. `fail_step` (1-5) makes that step error; `offline`
    /// makes the failure an `Unreachable` (as a down MCP server would).
    struct MockClient {
        fail_step: usize,
        offline: bool,
        run_status: &'static str,
    }

    impl MockClient {
        fn happy() -> Self {
            Self { fail_step: 0, offline: false, run_status: "completed" }
        }
        fn failing_at(step: usize) -> Self {
            Self { fail_step: step, offline: false, run_status: "completed" }
        }
        fn offline() -> Self {
            Self { fail_step: 1, offline: true, run_status: "completed" }
        }
        fn err(&self) -> McpError {
            if self.offline {
                McpError::Unreachable("connection refused".into())
            } else {
                McpError::Protocol("injected step failure".into())
            }
        }
    }

    impl ChaosLoopClient for MockClient {
        async fn discover(&self) -> Result<DiscoverOutcome, McpError> {
            if self.fail_step == 1 {
                return Err(self.err());
            }
            Ok(DiscoverOutcome { plugins: 12, actions: 34 })
        }
        async fn validate(&self, _experiment_path: &str) -> Result<ValidateOutcome, McpError> {
            if self.fail_step == 2 {
                return Err(self.err());
            }
            Ok(ValidateOutcome {
                valid: true,
                title: Some("Kill Postgres connections".into()),
                method_steps: 3,
                rollbacks: 1,
                summary: "Valid: 'Kill Postgres connections' — 3 method steps, 1 rollbacks".into(),
            })
        }
        async fn run_experiment(&self, _experiment_path: &str) -> Result<RunOutcome, McpError> {
            if self.fail_step == 3 {
                return Err(self.err());
            }
            Ok(RunOutcome {
                outcome: mcp::verdict_for(self.run_status).to_string(),
                status: self.run_status.to_string(),
                duration_ms: Some(228),
                journal_path: Some("/demo/journals/demo-postgres.toon".into()),
                ingestion: Some("ingested".into()),
            })
        }
        async fn analyze_store(&self, _query: &str) -> Result<TableOutcome, McpError> {
            if self.fail_step == 4 {
                return Err(self.err());
            }
            Ok(TableOutcome {
                columns: vec!["title".into(), "status".into(), "duration_ms".into()],
                rows: vec![vec!["Kill connections".into(), "completed".into(), "228".into()]],
                row_count: 1,
            })
        }
        async fn recommend(&self) -> Result<RecommendOutcome, McpError> {
            if self.fail_step == 5 {
                return Err(self.err());
            }
            Ok(RecommendOutcome {
                message: None,
                recommendations: vec![Recommendation {
                    rank: 1,
                    title: "Test Postgres failover".into(),
                    rationale: "never exercised".into(),
                }],
            })
        }
    }

    #[tokio::test]
    async fn happy_path_runs_all_five_steps_in_order() {
        let (ok, steps) = run_chaos_loop(&MockClient::happy(), "demo-postgres.toon").await;
        assert!(ok);
        assert_eq!(steps.len(), 5);
        let tools: Vec<&str> = steps.iter().map(|s| s.tool.as_str()).collect();
        assert_eq!(
            tools,
            vec![
                "tumult_discover",
                "tumult_validate",
                "tumult_run_experiment",
                "tumult_analyze_store",
                "tumult_recommend",
            ]
        );
        assert!(steps.iter().all(|s| s.status == "ok" && s.error.is_none()));
        // Discover step carries the counts.
        assert_eq!(steps[0].detail["plugins"], 12);
        // Recommend step surfaces the top recommendation title in its summary.
        assert!(steps[4].summary.contains("Test Postgres failover"));
    }

    #[tokio::test]
    async fn run_step_surfaces_halted_status() {
        let mut client = MockClient::happy();
        client.run_status = "halted";
        let (ok, steps) = run_chaos_loop(&client, "demo-postgres.toon").await;
        assert!(ok);
        let run = &steps[2];
        assert_eq!(run.detail["status"], "halted");
        assert_eq!(run.detail["outcome"], "halted");
        assert!(run.summary.starts_with("halted"));
    }

    #[tokio::test]
    async fn mid_loop_failure_stops_and_reports_failure() {
        let (ok, steps) = run_chaos_loop(&MockClient::failing_at(3), "demo-postgres.toon").await;
        assert!(!ok);
        // Steps 1-2 ran; step 3 errored; 4-5 never ran.
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[2].tool, "tumult_run_experiment");
        assert_eq!(steps[2].status, "error");
        assert!(steps[2].error.is_some());
        assert!(steps[0].status == "ok" && steps[1].status == "ok");
    }

    #[tokio::test]
    async fn mcp_offline_fails_cleanly_at_first_step() {
        let (ok, steps) = run_chaos_loop(&MockClient::offline(), "demo-postgres.toon").await;
        assert!(!ok);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].status, "error");
        assert!(steps[0]
            .error
            .as_deref()
            .unwrap()
            .to_lowercase()
            .contains("unreachable"));
    }
}
