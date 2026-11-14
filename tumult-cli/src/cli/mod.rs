//! Command-line interface definitions for the `tumult` binary.
//!
//! Holds the `clap`-derived argument parser types (`Cli`, `Commands`, and the
//! shared value enums). The per-command-group action enums live in the
//! sibling modules (`agentic`, `autopilot`, `chaosgraph`, `gameday`, `mcp`,
//! `store`, `topology`) and are re-exported here so `use crate::cli::*` keeps
//! working. The dispatch logic lives in the crate root (`main.rs`);
//! parser-behavior tests live in the `cli/tests/*` submodules.

#[cfg(test)]
mod tests;

mod agentic;
mod autopilot;
mod chaosgraph;
mod gameday;
mod mcp;
mod store;
mod topology;

use std::path::PathBuf;

use clap::Parser;

pub(crate) use tumult_cli::commands::{
    ComplianceFramework, ExportFormat, LoadToolArg, ReportFormat,
};

pub(crate) use agentic::AgenticAction;
pub(crate) use autopilot::AutopilotAction;
pub(crate) use chaosgraph::ChaosGraphAction;
pub(crate) use gameday::GameDayAction;
pub(crate) use mcp::{McpAction, McpTransport};
pub(crate) use store::StoreAction;
// Used by the parser tests via `crate::cli::*`; `main.rs` only calls methods
// on the value, so the bare re-export would warn as unused there.
pub(crate) use topology::TopologyAction;
#[allow(unused_imports)]
pub(crate) use topology::TopologyFormat;

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

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GateFailOn {
    /// Exit non-zero on a `fail` verdict only (default)
    Fail,
    /// Exit non-zero on `warn` or `fail`
    Warn,
    /// Always exit zero; print the verdict only
    Never,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerfFailOnArg {
    /// Exit 3 on a `regressed` verdict
    Regressed,
    /// Exit 4 on an `inconclusive` verdict, and 3 on a `regressed` one
    Inconclusive,
    /// Always exit zero; print the verdict only
    Never,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerfIncludeArg {
    /// Count the samples of aborted and truncated runs too
    Partial,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PerfQuantileArg {
    P50,
    P95,
    P99,
}

impl PerfQuantileArg {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::P50 => "p50",
            Self::P95 => "p95",
            Self::P99 => "p99",
        }
    }
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
    /// Run hypothesis checks and faults with declared tolerances (default)
    Full,
    /// Run with static tolerances (statistical baseline capture is not integrated)
    Skip,
    /// Measure steady-state probes once; exclude controls, faults, load and rollback
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
        /// Overwrite the journal file if it already exists
        #[arg(long)]
        force: bool,
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
        /// The version of the target this run measures (its
        /// `service.version`), recorded on the run as declared; without
        /// it and without `target.version` in the file, the version is
        /// observed from the target's telemetry when the run ends
        #[arg(long, value_name = "VERSION")]
        target_version: Option<String>,
        /// Run a load test concurrently with the experiment method
        #[arg(long, value_enum)]
        load: Option<LoadToolArg>,
        /// Path to load test script (k6 `.js`), or the URL to hit for oha
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
        /// Output path for the generated `.toon` (default: `<name>.toon`)
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
    /// Declared service topology, compliance lineage, and injection
    /// recommendations over the analytics store
    #[command(name = "topology")]
    Topology {
        #[command(subcommand)]
        action: TopologyAction,
    },
    /// Policy-gated autopilot: decide, record, and (only when told to)
    /// enact the next compliance-driven fault injections
    #[command(name = "autopilot")]
    Autopilot {
        #[command(subcommand)]
        action: AutopilotAction,
    },
    /// CI gate: ask the daemon for a target's resilience verdict and exit
    /// non-zero when it does not pass (`GET /api/gate/{target}`)
    Gate {
        /// Target system as recorded on the experiments (e.g. `payments-api`)
        target: String,
        /// Minimum acceptable score, 0-100 (daemon default 70)
        #[arg(long)]
        min_score: Option<f64>,
        /// Evidence older than this many days is stale (daemon default 30)
        #[arg(long)]
        max_age_days: Option<u32>,
        /// Restrict the verdict to one environment
        #[arg(long)]
        env: Option<String>,
        /// Daemon base URL (default: `TUMULT_DAEMON_URL`)
        #[arg(long)]
        daemon_url: Option<String>,
        /// Which verdicts make the command exit non-zero
        #[arg(long, value_enum, default_value_t = GateFailOn::Fail)]
        fail_on: GateFailOn,
        /// Print the daemon's JSON answer instead of the summary
        #[arg(long)]
        json: bool,
    },
    /// Performance gate: ask the daemon for a target's regression verdict
    /// between its latest two service.versions (`GET /api/perf/{target}`;
    /// `--store` posts to `/api/perf/{target}/verdicts`)
    Perf {
        /// Target system as recorded on the experiments (e.g. `payments-api`)
        target: String,
        /// Probe series to compare (daemon default `load.latency_p95_ms`)
        #[arg(long)]
        probe: Option<String>,
        /// Quantile the verdict is keyed on (daemon default p95)
        #[arg(long, value_enum)]
        quantile: Option<PerfQuantileArg>,
        /// Fewer samples on either side is inconclusive (daemon default 20)
        #[arg(long)]
        min_samples: Option<u32>,
        /// Fewer runs with samples on either side is inconclusive (daemon
        /// default 3, at most 1000); an interval from fewer than 5 is
        /// flagged as resting on few runs
        #[arg(long, value_parser = clap::value_parser!(u32).range(1..=1000))]
        min_runs: Option<u32>,
        /// Seed of the resampling (daemon default fixed): the same runs
        /// and seed give the same interval
        #[arg(long)]
        seed: Option<u64>,
        /// Noise margin as a percentage of the baseline quantile (daemon default 5)
        #[arg(long)]
        margin_pct: Option<f64>,
        /// The environment to compare in; needed when the target has
        /// versions in more than one
        #[arg(long)]
        env: Option<String>,
        /// Version to compare against (default: the one deployed just
        /// before the candidate)
        #[arg(long)]
        baseline: Option<String>,
        /// Version to judge (default: the one deployed last)
        #[arg(long)]
        candidate: Option<String>,
        /// Append the verdict to the daemon's `perf_verdicts` history
        /// (needs the operator role; a retry with the same evidence
        /// appends nothing and still succeeds); the default only reads
        #[arg(long)]
        store: bool,
        /// Daemon base URL (default: `TUMULT_DAEMON_URL`)
        #[arg(long)]
        daemon_url: Option<String>,
        /// How far back samples are read, such as `30d` or `12h` (daemon
        /// default 30d); the order of the versions does not depend on it
        #[arg(long)]
        range: Option<String>,
        /// Runs left out by default that are to be counted
        #[arg(long, value_enum)]
        include: Option<PerfIncludeArg>,
        /// Which verdicts make the command exit non-zero, comma-separated:
        /// `regressed` exits 3, `inconclusive` exits 4
        #[arg(
            long,
            value_enum,
            value_delimiter = ',',
            default_values_t = [PerfFailOnArg::Regressed, PerfFailOnArg::Inconclusive]
        )]
        fail_on: Vec<PerfFailOnArg>,
        /// Print the daemon's JSON answer instead of the summary
        #[arg(long)]
        json: bool,
    },
    /// Open the interactive analytics TUI over the store (read-only dashboard)
    #[command(alias = "dashboard")]
    Tui {
        /// Analytics store path (default: ~/.tumult/lake.duckdb, override with
        /// `TUMULT_LAKE_PATH`)
        #[arg(long)]
        store: Option<PathBuf>,
        /// Live-refresh interval in seconds (minimum 1)
        #[arg(long, default_value_t = 2)]
        refresh_secs: u64,
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
