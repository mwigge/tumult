//! Runtime configuration, shared application state, and the demo's static
//! fault-domain catalog.

use serde_json::json;

use crate::mcp::McpClient;

/// SQL the Analytics card runs over the persistent store: the most recent
/// experiments with title, status and duration.
pub(crate) const ANALYTICS_SQL: &str =
    "SELECT title, status, duration_ms FROM experiments ORDER BY started_at_ns DESC LIMIT 8";

/// Default fault domain the chaos loop validates and runs when none is given.
pub(crate) const DEFAULT_LOOP_DOMAIN: &str = "postgres";

/// The auto-halt guardrail experiment. Kept out of the pass/fail sweep: its
/// expected outcome is `Halted` (the guard pulls the run mid-flight), not
/// `Completed`, so it has its own "Safety guardrail" card.
pub(crate) const GUARD_HALT_DOMAIN: &str = "guard-halt";

/// The demo fault domains from CONTRACT.md (plus the two timewarp domains), in
/// display order. Each runs `demo-<id>.toon` and completes on success, so they
/// double as the fault sweep and the per-domain cards.
pub(crate) const DOMAINS: &[(&str, &str)] = &[
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
pub(crate) struct Config {
    /// MCP base URL, e.g. `http://tumult-mcp:3100` (path `/mcp` appended by client).
    pub(crate) mcp_url: String,
    /// Directory the experiments are mounted at *inside the tumult-mcp container*.
    pub(crate) experiments_dir: String,
    /// SigNoz base URL for the "View traces" deep link.
    pub(crate) signoz_url: String,
    /// Demo app URL for the top status bar link.
    pub(crate) demo_app_url: String,
    /// Service name to filter traces by in SigNoz.
    pub(crate) trace_service: String,
    /// Directory (inside the tumult-mcp container) the fault sweep writes its
    /// journals into — the corpus the Compliance card evaluates.
    pub(crate) journals_dir: String,
    /// Regulatory framework the Compliance / ChaosGraph cards report against.
    pub(crate) compliance_framework: String,
    /// Bind port.
    pub(crate) port: u16,
}

impl Config {
    /// Build from the environment. Every field takes a **neutral** `TUMULT_UI_*`
    /// name first so the same binary runs against any tumult-mcp, then falls
    /// back to the demo's `DEMO_*` / legacy names so the demo compose keeps
    /// working unchanged. `MCP_URL` and `TUMULT_MCP_TOKEN` are already neutral.
    pub(crate) fn from_env() -> Self {
        let mcp_url = env_chain(&["TUMULT_UI_MCP_URL", "MCP_URL"], "http://tumult-mcp:3100");
        let experiments_dir = trim_trailing_slash(&env_chain(
            &["TUMULT_UI_EXPERIMENTS_DIR", "DEMO_EXPERIMENTS_DIR"],
            "/demo/experiments",
        ));
        Self {
            mcp_url,
            experiments_dir,
            signoz_url: trim_trailing_slash(&env_chain(
                &["TUMULT_UI_OBSERVABILITY_URL", "SIGNOZ_URL"],
                "http://localhost:3301",
            )),
            demo_app_url: trim_trailing_slash(&env_chain(
                &["TUMULT_UI_TARGET_URL", "DEMO_APP_URL"],
                "http://localhost:8080",
            )),
            trace_service: env_chain(&["TUMULT_UI_TARGET_HINT", "TRACE_SERVICE"], "demo-app"),
            journals_dir: trim_trailing_slash(&env_chain(
                &["TUMULT_UI_JOURNALS_DIR", "DEMO_JOURNALS_DIR"],
                "/journals",
            )),
            compliance_framework: env_chain(
                &["TUMULT_UI_FRAMEWORK", "DEMO_COMPLIANCE_FRAMEWORK"],
                "dora",
            ),
            port: env_chain(&["TUMULT_UI_PORT", "PORT"], "8088")
                .parse()
                .unwrap_or(8088),
        }
    }

    /// Experiment path for a domain, as seen inside the tumult-mcp container.
    pub(crate) fn experiment_path(&self, domain: &str) -> String {
        format!("{}/demo-{}.toon", self.experiments_dir, domain)
    }

    /// SigNoz deep link to the traces explorer. SigNoz's exact pre-filter query
    /// params vary across versions, so we link to the traces-explorer page
    /// (which always exists on the standalone build) and the UI states the
    /// service to filter on (`demo-app`) explicitly — a working link and a
    /// clear filter instruction rather than a fragile pre-filter that may 404.
    pub(crate) fn signoz_trace_link(&self) -> String {
        format!("{}/traces-explorer", self.signoz_url)
    }
}

/// Shared application state.
pub(crate) struct AppState {
    pub(crate) cfg: Config,
    pub(crate) client: McpClient,
    /// Page HTML with server-side config substituted in.
    pub(crate) index_html: String,
}

/// First non-empty value among `keys` (in order), else `default`. Lets a
/// neutral `TUMULT_UI_*` name take precedence over a demo-specific fallback.
fn env_chain(keys: &[&str], default: &str) -> String {
    keys.iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| default.to_string())
}

fn trim_trailing_slash(s: &str) -> String {
    s.trim_end_matches('/').to_string()
}

/// Inject the runtime config into the static HTML as a JSON blob the page's JS
/// reads on load, then hand back the full document.
pub(crate) fn render_index(cfg: &Config) -> String {
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
