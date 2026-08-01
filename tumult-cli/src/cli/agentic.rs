//! `agentic` subcommand arguments.

use std::path::PathBuf;

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
