//! Axum request handlers for the control-panel HTTP API.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::chaos_loop::{run_chaos_loop, LoopReport};
use crate::mcp::{self, McpError, ScaffoldArgs};
use crate::state::{AppState, ANALYTICS_SQL, DEFAULT_LOOP_DOMAIN, DOMAINS, GUARD_HALT_DOMAIN};

// ── Handlers ──────────────────────────────────────────────────

pub(crate) async fn serve_index(State(state): State<Arc<AppState>>) -> Html<String> {
    Html(state.index_html.clone())
}

pub(crate) async fn healthz() -> impl IntoResponse {
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
pub(crate) struct StatusView {
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
pub(crate) async fn api_status(State(state): State<Arc<AppState>>) -> Json<StatusView> {
    let (mcp_online, mcp_error, tool_count, run_destructive) = match state.client.list_tools().await
    {
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

/// The caller's role as the browser sees it. Always 200: a whoami failure
/// degrades to least privilege (`viewer`) with `resolved: false` and a note, so
/// the UI enforces the tighter tier rather than assuming operator.
#[derive(Serialize)]
pub(crate) struct WhoamiView {
    /// Resolved role: `viewer` or `operator` (falls back to `viewer` on error).
    role: String,
    /// Whether a configured bearer token authenticated the request.
    authenticated: bool,
    /// True when the role came back from the server; false when it was assumed.
    resolved: bool,
    /// Present only when the role could not be resolved.
    error: Option<String>,
}

/// Role-awareness endpoint: `tumult_whoami` over MCP. The UI calls this on load
/// to render a role badge and hide operator-only actions from viewers — defense
/// in depth over the server's RBAC, which still enforces regardless. A failure
/// is reported in-band as `viewer` (least privilege), never a panic.
pub(crate) async fn api_whoami(State(state): State<Arc<AppState>>) -> Json<WhoamiView> {
    match state.client.whoami().await {
        Ok(who) => Json(WhoamiView {
            role: who.role,
            authenticated: who.authenticated,
            resolved: true,
            error: None,
        }),
        Err(e) => {
            tracing::warn!("whoami failed: {e}");
            Json(WhoamiView {
                role: "viewer".to_string(),
                authenticated: false,
                resolved: false,
                error: Some(e.to_string()),
            })
        }
    }
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
pub(crate) async fn api_run(
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
        McpError::Rpc(_) | McpError::Protocol(_) | McpError::Transport(_) => {
            StatusCode::BAD_GATEWAY
        }
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
pub(crate) async fn api_guardrail(State(state): State<Arc<AppState>>) -> Response {
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
pub(crate) async fn api_compliance(State(state): State<Arc<AppState>>) -> Response {
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
pub(crate) async fn api_analytics(State(state): State<Arc<AppState>>) -> Response {
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
            Ok(data) => Self {
                data: Some(data),
                error: None,
            },
            Err(e) => Self {
                data: None,
                error: Some(e.to_string()),
            },
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
pub(crate) async fn api_chaosgraph(State(state): State<Arc<AppState>>) -> Response {
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
        Err(e) => (
            None,
            GraphSection {
                data: None,
                error: Some(e.to_string()),
            },
        ),
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

// ── New experiment (web authoring) ────────────────────────────
// The "New experiment" card lets an SRE pick a fault from the live catalog and
// scaffold a runnable experiment — the same authoring path the CLI (`tumult
// new`) and an agent drive, exposed in the browser. Both routes go through the
// shared MCP client (auth injected by `call_tool`).

/// New-experiment picker: `tumult_fault_catalog` → the domains/actions/args tree
/// the card populates its dropdowns and dynamic arg inputs from. MCP failures →
/// a clean JSON error so the card degrades in-band instead of breaking the page.
pub(crate) async fn api_catalog(State(state): State<Arc<AppState>>) -> Response {
    match state.client.fault_catalog().await {
        Ok(out) => Json(json!({
            "action_count": out.action_count,
            "domains": out.domains,
        }))
        .into_response(),
        Err(e) => {
            tracing::warn!("catalog failed: {e}");
            error_response(status_for(&e), &e.to_string())
        }
    }
}

/// JSON body for `POST /api/scaffold`: the picker's selection. All fields but
/// `action` + `target` are optional; empty strings are treated as absent.
#[derive(Deserialize, Default)]
pub(crate) struct ScaffoldBody {
    plugin: Option<String>,
    action: String,
    #[serde(default)]
    args: serde_json::Value,
    target: String,
    probe_command: Option<String>,
    probe_url: Option<String>,
    probe_expect: Option<String>,
    title: Option<String>,
}

/// Scaffold an experiment from the picker's selection via
/// `tumult_scaffold_experiment`. Returns `{action, toon, valid,
/// validation_error, run_hint}` for copy/paste — the demo mounts the experiments
/// dir read-only, so the card shows the TOON and a `tumult run` hint rather than
/// writing or auto-running arbitrary generated experiments. A scaffold that
/// fails validation is a 200 with `valid: false` (badged by the card), not an
/// error; only a true MCP/tool failure returns an error status.
pub(crate) async fn api_scaffold(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ScaffoldBody>,
) -> Response {
    if body.action.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "action is required");
    }
    if body.target.trim().is_empty() {
        return error_response(StatusCode::BAD_REQUEST, "target is required");
    }
    let req = ScaffoldArgs {
        plugin: body.plugin,
        action: body.action,
        args: body.args,
        target: body.target,
        probe_command: body.probe_command,
        probe_url: body.probe_url,
        probe_expect: body.probe_expect,
        title: body.title,
    };
    match state.client.scaffold_experiment(req).await {
        Ok(out) => Json(json!({
            "action": out.action,
            "toon": out.toon,
            "valid": out.valid,
            "validation_error": out.validation_error,
            "run_hint": "Save the TOON to a file, then run it: tumult run <file>",
        }))
        .into_response(),
        Err(e) => {
            tracing::warn!("scaffold failed: {e}");
            error_response(status_for(&e), &e.to_string())
        }
    }
}

// ── Chaos loop showcase ───────────────────────────────────────

/// Query params for the loop endpoint: an optional fault `domain` selecting the
/// experiment to validate and run (defaults to `postgres`).
#[derive(Deserialize, Default)]
pub(crate) struct LoopParams {
    domain: Option<String>,
}

/// Run the whole chaos loop via MCP and return every step's result. Always
/// returns 200 with a body — an offline MCP server or a mid-loop failure is
/// reported in-band as a failed step, never a panic. Unknown domain → 404.
pub(crate) async fn api_loop(
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
