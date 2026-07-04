//! Command-line interface definitions for the `tumult` binary.
//!
//! Holds the `clap`-derived argument parser types (`Cli`, `Commands`, and the
//! subcommand/value enums). The dispatch logic lives in the crate root
//! (`main.rs`); parser-behavior tests live in the `cli/tests/*` submodules.

#[cfg(test)]
mod tests;

use std::path::PathBuf;

use clap::Parser;

pub(crate) use tumult_cli::commands::{
    ComplianceFramework, ExportFormat, LoadToolArg, ReportFormat,
};

#[derive(Parser, Debug)]
#[command(
    name = "tumult",
    version,
    propagate_version = true,
    about = "Rust-native chaos engineering platform"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,
}

/// Maps to `tumult_core::execution::RollbackStrategy`
#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
pub(crate) enum RollbackStrategy {
    Always,
    #[value(alias = "deviated")]
    OnDeviation,
    Never,
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
pub(crate) enum OutputFormat {
    /// Print journal as JSON to stdout
    Json,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecommendFormat {
    Text,
    Json,
}

impl From<RecommendFormat> for tumult_intelligence::OutputFormat {
    fn from(format: RecommendFormat) -> Self {
        match format {
            RecommendFormat::Text => Self::Text,
            RecommendFormat::Json => Self::Json,
        }
    }
}

#[derive(clap::ValueEnum, Clone, Debug, PartialEq, Eq)]
pub(crate) enum BaselineMode {
    /// Run full baseline then inject fault (default)
    Full,
    /// Skip baseline, use static tolerances
    Skip,
    /// Run baseline only, no fault injection
    Only,
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum Commands {
    /// Execute a chaos experiment
    Run {
        /// Path to experiment .toon file
        experiment: PathBuf,
        /// Output journal location
        #[arg(long, default_value = "journal.toon")]
        journal_path: PathBuf,
        /// Validate and show plan without executing
        #[arg(long)]
        dry_run: bool,
        /// Rollback strategy
        #[arg(long, default_value_t = RollbackStrategy::OnDeviation, value_enum)]
        rollback_strategy: RollbackStrategy,
        /// Baseline mode
        #[arg(long, default_value_t = BaselineMode::Full, value_enum)]
        baseline_mode: BaselineMode,
        /// Skip auto-ingestion into persistent analytics store
        #[arg(long)]
        no_ingest: bool,
        /// Output format for journal (human-readable summary or JSON to stdout)
        #[arg(long, value_enum)]
        output_format: Option<OutputFormat>,
        /// Template variable substitution: KEY=VALUE (may be repeated)
        #[arg(long = "var", value_name = "KEY=VALUE", action = clap::ArgAction::Append)]
        vars: Vec<String>,
        /// Run a load test concurrently with the experiment method
        #[arg(long, value_enum)]
        load: Option<LoadToolArg>,
        /// Path to load test script (k6 `.js` or `JMeter` `.jmx`)
        #[arg(long)]
        load_script: Option<PathBuf>,
        /// Number of virtual users for load test
        #[arg(long)]
        load_vus: Option<u32>,
        /// Load test duration (e.g. "30s", "5m")
        #[arg(long)]
        load_duration: Option<String>,
    },
    /// Validate experiment syntax and plugin references
    Validate {
        /// Path to experiment .toon file
        experiment: PathBuf,
    },
    /// List all discovered plugins, actions, and probes
    Discover {
        /// Show details for a specific plugin
        #[arg(long)]
        plugin: Option<String>,
    },
    /// SQL analytics over journal files
    Analyze {
        /// Directory containing journal files (omit to use persistent store)
        journals: Option<PathBuf>,
        /// SQL query to execute (raw SQL mode)
        #[arg(long)]
        query: Option<String>,
        /// Show summary of last N experiments (default: 1 if no --query)
        #[arg(long)]
        last: Option<usize>,
        /// Show store-wide aggregate summary
        #[arg(long)]
        all: bool,
    },
    /// Convert journal to other formats
    Export {
        /// Journal file to export
        journal: PathBuf,
        /// Output format
        #[arg(long, default_value_t = ExportFormat::Parquet, value_enum)]
        format: ExportFormat,
    },
    /// Regulatory compliance report
    Compliance {
        /// Directory containing journal files. Optional when `--sources` is
        /// given (which lists the sourced, dated citation registry only).
        #[arg(required_unless_present = "sources")]
        journals: Option<PathBuf>,
        /// Target regulatory framework
        #[arg(long, value_enum)]
        framework: ComplianceFramework,
        /// List every mapped citation with its official source URL and
        /// last-verified date, then exit without analysing journals. Makes
        /// citation drift auditable.
        #[arg(long)]
        sources: bool,
    },
    /// Generate report from journal (HTML or PDF)
    Report {
        /// Journal file
        journal: PathBuf,
        /// Output path
        #[arg(long)]
        output: Option<PathBuf>,
        /// Report format
        #[arg(long, default_value_t = ReportFormat::Html, value_enum)]
        format: ReportFormat,
        /// Base URL of a trace UI (e.g. Jaeger/Tempo). When set, HTML reports
        /// render each activity's `trace_id` as a clickable link. Falls back to
        /// the `TUMULT_TRACE_UI_BASE` env var (resolved in `cmd_report`). Off by
        /// default.
        #[arg(long)]
        trace_ui_base: Option<String>,
    },
    /// Cross-run trend analysis
    Trend {
        /// Directory containing journal files
        journals: PathBuf,
        /// Metric to track
        #[arg(long, default_value = "resilience_score")]
        metric: String,
        /// Time window (e.g., 30d, 90d)
        #[arg(long)]
        last: Option<String>,
        /// Filter by target technology (matches experiment title)
        #[arg(long)]
        target: Option<String>,
    },
    /// Scaffold a new experiment.toon from a bundled template
    Init {
        /// Plugin name to reference in the generated template
        #[arg(long)]
        plugin: Option<String>,
    },
    /// Pick a fault and get a validated, ready-to-run experiment
    ///
    /// With no flags this is an interactive picker (domain → action → args →
    /// target → probe → title). With `--from <template>` it instantiates a
    /// curated starter non-interactively. See `tumult templates`.
    New {
        /// Curated starter template to instantiate (non-interactive)
        #[arg(long)]
        from: Option<String>,
        /// Parameter override for `--from`: KEY=VALUE (may be repeated)
        #[arg(long = "set", value_name = "KEY=VALUE", action = clap::ArgAction::Append)]
        set: Vec<String>,
        /// Output path for the generated `.toon` (default: <name>.toon)
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// List the curated starter templates (name, description, params)
    Templates,
    /// Import journals from Parquet backup
    Import {
        /// Directory containing Parquet backup files
        parquet_dir: PathBuf,
    },
    /// Persistent analytics store management
    Store {
        #[command(subcommand)]
        action: StoreAction,
    },
    /// AI-assisted recommendation for the next useful chaos experiment
    Recommend {
        /// Recommendation goal or operator intent
        #[arg(long)]
        goal: Option<String>,
        /// Analytics store path to inspect
        #[arg(long)]
        store_path: Option<PathBuf>,
        /// Model label to include in deterministic recommendation metadata
        #[arg(long)]
        model: Option<String>,
        /// Do not include a draft TOON experiment
        #[arg(long)]
        no_draft: bool,
        /// Output format
        #[arg(long, default_value_t = RecommendFormat::Text, value_enum)]
        format: RecommendFormat,
        /// Enhance recommendations with an agent CLI adapter (e.g.
        /// claude-code, codex); see `tumult agents` for detected adapters
        #[arg(long)]
        agent: Option<String>,
        /// Model override passed to the agent CLI (requires --agent)
        #[arg(long, requires = "agent")]
        agent_model: Option<String>,
        /// Agent CLI timeout in seconds
        #[arg(long, default_value_t = tumult_agent_cli::adapter::DEFAULT_TIMEOUT.as_secs())]
        agent_timeout: u64,
        /// Write validated agent-proposed experiments (.toon) into this
        /// directory (requires --agent)
        #[arg(long, value_name = "DIR", requires = "agent")]
        generate_experiments: Option<PathBuf>,
    },
    /// List agent CLI adapters (install, version, and auth state)
    Agents,
    /// Agentic AI fault-injection scenarios and local smoke tests
    Agentic {
        #[command(subcommand)]
        action: AgenticAction,
    },
    /// Coordinated experiment campaigns with resilience scoring
    #[command(name = "gameday")]
    GameDay {
        #[command(subcommand)]
        action: GameDayAction,
    },
    /// Model Context Protocol server exposing Tumult as tools for AI agents
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    // `ChaosGraph` is a product name rendered verbatim in `--help`; backticks
    // would leak into the help text, so silence the doc-markdown lint here.
    #[allow(clippy::doc_markdown)]
    /// Query the ChaosGraph knowledge graph over the analytics store
    #[command(name = "chaosgraph")]
    ChaosGraph {
        #[command(subcommand)]
        action: ChaosGraphAction,
    },
}

/// Text vs. structured JSON rendering for `chaosgraph` output.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GraphFormat {
    /// Readable, indented text summary (default)
    Text,
    /// The underlying structured object as pretty JSON
    Json,
}

/// Transport for `tumult mcp serve`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum McpTransport {
    /// JSON-RPC over stdin/stdout (default)
    Stdio,
    /// Streamable HTTP / SSE
    Http,
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum McpAction {
    /// Start the MCP server (stdio for local agents, HTTP for networked use)
    Serve {
        /// Transport mode
        #[arg(long, default_value_t = McpTransport::Stdio, value_enum)]
        transport: McpTransport,
        /// Bind address for the HTTP transport and health endpoint. Loopback by
        /// default; a non-loopback bind (e.g. 0.0.0.0) requires --token.
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Port for the HTTP transport
        #[arg(long, default_value_t = 3100)]
        port: u16,
        /// Port for the /health endpoint (default: port + 1)
        #[arg(long)]
        health_port: Option<u16>,
        /// Require this bearer token on every request (sets `TUMULT_MCP_TOKEN`,
        /// mapped to the `operator` role)
        #[arg(long)]
        token: Option<String>,
        /// Path to a TOML auth config file granting per-token roles (viewer /
        /// operator). Overrides --token; sets `TUMULT_MCP_AUTH_CONFIG`.
        #[arg(long)]
        auth_config: Option<std::path::PathBuf>,
    },
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum ChaosGraphAction {
    /// List graph nodes of a given kind (experiment, fault, service, journal, …)
    Query {
        /// Node kind to list
        #[arg(long)]
        kind: String,
        /// Case-insensitive label substring filter
        #[arg(long)]
        filter: Option<String>,
        /// Output format
        #[arg(long, value_enum, default_value_t = GraphFormat::Text)]
        format: GraphFormat,
        /// Analytics store path (default: ~/.tumult/analytics.duckdb)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Show the ego sub-graph (neighbours) around a node
    Neighbors {
        /// Node id to center on (e.g. `exp:My experiment`)
        #[arg(long)]
        node: String,
        /// Restrict to a single relation (e.g. `injects`, `targets`)
        #[arg(long)]
        rel: Option<String>,
        /// Traversal depth in hops
        #[arg(long, default_value_t = 1)]
        depth: u32,
        /// Output format
        #[arg(long, value_enum, default_value_t = GraphFormat::Text)]
        format: GraphFormat,
        /// Analytics store path (default: ~/.tumult/analytics.duckdb)
        #[arg(long)]
        store: Option<PathBuf>,
    },
    /// Report chaos actions never exercised by a tested run
    #[command(name = "coverage-gaps")]
    CoverageGaps {
        /// Annotate with this framework's still-unevidenced articles
        #[arg(long)]
        framework: Option<String>,
        /// Filter gaps to a fault domain (plugin name substring)
        #[arg(long)]
        domain: Option<String>,
        /// Also persist the derived gap sub-graph into the store so
        /// `chaosgraph query/neighbors` can navigate it. Takes a write lock, so
        /// it conflicts with a running MCP server on the same store.
        #[arg(long)]
        refresh: bool,
        /// Output format
        #[arg(long, value_enum, default_value_t = GraphFormat::Text)]
        format: GraphFormat,
        /// Analytics store path (default: ~/.tumult/analytics.duckdb)
        #[arg(long)]
        store: Option<PathBuf>,
    },
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum AgenticAction {
    /// List bundled agentic scenario packs
    #[command(name = "list-packs")]
    ListPacks,
    /// Run the deterministic local malformed-output smoke path
    Smoke {
        /// Metadata-only journal output path
        #[arg(long, default_value = "target/agentic/smoke-journal.toon")]
        journal: PathBuf,
    },
    /// Run a bundled scenario pack with deterministic local fixtures
    Run {
        /// Bundled scenario pack name
        #[arg(long, default_value = "malformed-json-recovery")]
        scenario: String,
        /// Metadata-only journal output path
        #[arg(long, default_value = "target/agentic/run-journal.toon")]
        journal: PathBuf,
    },
    /// Run a bundled multi-turn trajectory pack (agent-graph fault modeling)
    ///
    /// Injects a fault at a specific step of an ordered model+tool trajectory and
    /// evaluates whole-trajectory contracts (recovery, loop-avoidance,
    /// termination, step budget) plus per-dimension agentic subscores. Runs
    /// entirely against in-process metadata baselines — no network.
    Trajectory {
        /// Bundled trajectory pack name
        #[arg(long, default_value = "rag-grounding-failure")]
        pack: String,
        /// Metadata-only journal output path
        #[arg(long, default_value = "target/agentic/trajectory-journal.toon")]
        journal: PathBuf,
    },
    /// Run deterministic replay fixture validation
    Replay {
        /// Replay fixture path
        #[arg(
            long,
            default_value = "examples/agentic/malformed-json-recovery.fixture.json"
        )]
        fixture: PathBuf,
        /// Metadata-only journal output path
        #[arg(long, default_value = "target/agentic/replay-journal.toon")]
        journal: PathBuf,
    },
    /// Inject a scenario pack's faults into a live agent's model traffic
    ///
    /// Stands up a local reverse proxy in front of a provider endpoint; point
    /// any base-URL-configurable agent (Claude Code, Codex, Copilot, and others)
    /// at it via its base-URL or proxy environment variable.
    Proxy {
        /// Address to listen on — set your agent's base URL to this
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: String,
        /// Upstream provider base URL to forward to
        #[arg(long, default_value = "https://api.anthropic.com")]
        upstream: String,
        /// Scenario pack whose faults are injected into live traffic
        #[arg(long, default_value = "malformed-json-recovery")]
        scenario: String,
        /// Optional JSONL journal: one line appended per proxied request
        #[arg(long)]
        journal: Option<PathBuf>,
        /// Base seed for the per-request fault gate
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Client targeted by the proxy: claude-code, codex, copilot, opencode
        #[arg(long, default_value = "unknown")]
        client: String,
    },
    /// Orchestrate a live agent run with tumult as the trace root
    ///
    /// Starts a tumult.experiment root span, runs `claude -p` with that trace
    /// context + telemetry export + a base URL pointing at the proxy, and
    /// evaluates the scenario pack's contracts against the agent's response.
    RunLive {
        /// Prompt to send to the agent
        #[arg(long)]
        prompt: String,
        /// Scenario pack whose contracts are evaluated against the response
        #[arg(long, default_value = "malformed-json-recovery")]
        scenario: String,
        /// Base URL the agent should use (point at the tumult proxy)
        #[arg(long, default_value = "http://127.0.0.1:8080")]
        base_url: String,
        /// Optional OTLP endpoint for the agent's telemetry export
        #[arg(long)]
        otlp: Option<String>,
        /// Client being orchestrated (tags telemetry)
        #[arg(long, default_value = "claude-code")]
        client: String,
    },
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum StoreAction {
    /// Show store statistics
    Stats,
    /// Export entire store to Parquet backup
    Backup {
        /// Output directory for backup files
        #[arg(long, default_value = "tumult-backup")]
        output: PathBuf,
    },
    /// Purge experiments older than N days
    Purge {
        /// Number of days to retain
        #[arg(long)]
        older_than_days: u32,
    },
    /// Show store file path
    Path,
    /// Migrate data from `DuckDB` to `ClickHouse`
    Migrate,
}

#[derive(clap::Subcommand, Debug)]
pub(crate) enum GameDayAction {
    /// Create a `.gameday.toon` file from experiment paths
    Create {
        /// Name for the `GameDay`
        name: String,
        /// Experiment `.toon` files (comma-separated)
        #[arg(long, value_delimiter = ',')]
        experiments: Vec<PathBuf>,
        /// Load tool to run during the `GameDay`
        #[arg(long, value_enum)]
        load: Option<LoadToolArg>,
        /// Path to load test script
        #[arg(long)]
        load_script: Option<PathBuf>,
        /// Number of virtual users
        #[arg(long)]
        load_vus: Option<u32>,
        /// Compliance framework to map
        #[arg(long)]
        framework: Option<ComplianceFramework>,
    },
    /// Run all experiments in a `GameDay` under shared load
    Run {
        /// Path to `.gameday.toon` file
        gameday: PathBuf,
    },
    /// Show aggregate analysis of a completed `GameDay`
    Analyze {
        /// Path to `.gameday.toon` file (uses journals from same directory)
        gameday: PathBuf,
    },
}
