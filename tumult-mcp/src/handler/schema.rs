//! MCP tool schema definitions and their default-value helpers.

use rust_mcp_sdk::macros;

// ── Tool schema definitions ───────────────────────────────────

#[macros::mcp_tool(
    name = "tumult_run_experiment",
    description = "Execute a Tumult chaos experiment. Persists the journal (journal_path, default journal.toon in the workspace root), auto-ingests it into the analytics store unless no_ingest is set, and returns the run as JSON (format=json, default) or the journal as TOON (format=toon).",
    destructive_hint = true,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = true
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct RunExperimentTool {
    pub experiment_path: String,
    /// One of `on-deviation` (default), `always`, `never`.
    #[serde(default = "default_strategy")]
    pub rollback_strategy: String,
    /// Where to write the journal, relative to the workspace root.
    /// Defaults to `journal.toon` (CLI parity).
    pub journal_path: Option<String>,
    /// Skip analytics-store ingestion (parity with the CLI `--no-ingest`).
    #[serde(default)]
    pub no_ingest: bool,
    /// Analytics store the journal is ingested into.
    #[serde(default = "default_store_path")]
    pub store_path: String,
    /// Text content format: `json` (default) or `toon`.
    #[serde(default = "default_journal_format")]
    pub format: String,
}
fn default_strategy() -> String {
    "on-deviation".into()
}
fn default_journal_format() -> String {
    "json".into()
}

#[macros::mcp_tool(
    name = "tumult_validate",
    description = "Validate an experiment file for syntax and provider support.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ValidateTool {
    pub experiment_path: String,
}

#[macros::mcp_tool(
    name = "tumult_analyze",
    description = "SQL query over experiment journals via embedded DuckDB.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AnalyzeTool {
    pub journals_path: String,
    pub query: String,
}

#[macros::mcp_tool(
    name = "tumult_read_journal",
    description = "Read a journal file and return it as JSON (format=json, default) or raw TOON (format=toon). Set summary=true for a compact summary instead of the full journal. Text content is capped at 512 KiB.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ReadJournalTool {
    pub journal_path: String,
    /// Text content format: `json` (default) or `toon`.
    #[serde(default = "default_journal_format")]
    pub format: String,
    /// Return only a compact summary (title, status, timing, counts).
    #[serde(default)]
    pub summary: bool,
}

#[macros::mcp_tool(
    name = "tumult_list_journals",
    description = "List .toon journal files in a directory (sorted). Supports limit (default 100, max 1000) and offset; structured content is {items, total, offset, limit}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ListJournalsTool {
    /// Directory to list. `directory` is accepted as a legacy alias.
    #[serde(alias = "directory")]
    pub path: String,
    /// Maximum number of entries returned (default 100, max 1000).
    #[serde(default = "default_list_limit")]
    pub limit: u64,
    /// Number of entries to skip before the returned page.
    #[serde(default)]
    pub offset: u64,
}
pub(crate) fn default_list_limit() -> u64 {
    100
}

#[macros::mcp_tool(
    name = "tumult_discover",
    description = "List all Tumult plugins, actions, and probes.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct DiscoverTool {}

#[macros::mcp_tool(
    name = "tumult_create_experiment",
    description = "Create a new experiment from a template.",
    destructive_hint = false,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct CreateExperimentTool {
    pub output_path: String,
    pub plugin: Option<String>,
}

#[macros::mcp_tool(
    name = "tumult_query_traces",
    description = "Query trace data from a journal — returns activity spans with trace/span IDs for observability correlation.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct QueryTracesTool {
    pub journal_path: String,
}

#[macros::mcp_tool(
    name = "tumult_store_stats",
    description = "Get persistent analytics store statistics — experiment count, activity count, schema version, file size.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct StoreStatsTool {
    #[serde(default = "default_store_path")]
    pub store_path: String,
}
pub(crate) fn default_store_path() -> String {
    let path = tumult_analytics::AnalyticsStore::default_path();
    path.to_str().map_or_else(
        || ".tumult/analytics.db".to_string(),
        std::string::ToString::to_string,
    )
}

#[macros::mcp_tool(
    name = "tumult_analyze_store",
    description = "SQL query over the persistent analytics store (accumulated history from all runs).",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AnalyzeStoreTool {
    pub query: String,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

#[macros::mcp_tool(
    name = "tumult_list_experiments",
    description = "List all .toon experiment files recursively from the workspace or a given path (sorted by relative path). Supports limit (default 100, max 1000) and offset; structured content is {items, total, offset, limit}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ListExperimentsTool {
    /// Optional subdirectory to search within (relative to workspace root).
    pub path: Option<String>,
    /// Maximum number of entries returned (default 100, max 1000).
    #[serde(default = "default_list_limit")]
    pub limit: u64,
    /// Number of entries to skip before the returned page.
    #[serde(default)]
    pub offset: u64,
}

#[macros::mcp_tool(
    name = "tumult_report",
    description = "Render an experiment journal as a report: format=json (raw journal JSON, default) or format=junit (JUnit XML, one testcase per activity). With output_path the report is written inside the workspace and the path is returned; otherwise the content is returned inline (capped at 512 KiB). HTML/PDF reports are CLI-only.",
    destructive_hint = false,
    read_only_hint = false,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ReportTool {
    /// Path to the journal `.toon` file.
    pub journal_path: String,
    /// Report format: `json` (default) or `junit`.
    #[serde(default = "default_journal_format")]
    pub format: String,
    /// Where to write the report, relative to the workspace root. When
    /// omitted the report content is returned inline.
    pub output_path: Option<String>,
}

#[macros::mcp_tool(
    name = "tumult_compliance",
    description = "Regulatory compliance summary over journals (a .toon journal file or a directory of them): pass rate, recovery compliance, and verdict for a target framework. Valid frameworks: dora, nis2, pci-dss, iso-22301, iso-27001, soc2, basel-iii.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ComplianceTool {
    /// Journal `.toon` file or directory containing journal files.
    pub journals_path: String,
    /// Target framework: one of `dora`, `nis2`, `pci-dss`, `iso-22301`,
    /// `iso-27001`, `soc2`, `basel-iii`.
    pub framework: String,
}

#[macros::mcp_tool(
    name = "tumult_trend",
    description = "Cross-run metric trend over journals (a .toon journal file or a directory of them): metric one of resilience_score (default), duration_ms, estimate_accuracy, method_step_count; optional last window (e.g. 30d) and target title filter. Returns time-ordered points and a direction verdict.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct TrendTool {
    /// Journal `.toon` file or directory containing journal files.
    pub journals_path: String,
    /// Metric to track: `resilience_score` (default), `duration_ms`,
    /// `estimate_accuracy`, or `method_step_count`.
    #[serde(default = "default_trend_metric")]
    pub metric: String,
    /// Time window in days (e.g. `30d`).
    pub last: Option<String>,
    /// Case-insensitive experiment-title substring filter.
    pub target: Option<String>,
}
fn default_trend_metric() -> String {
    "resilience_score".into()
}

#[macros::mcp_tool(
    name = "tumult_agents",
    description = "List agent CLI adapters (claude-code, codex) with install, version, and auth state. Probes local binaries by spawning short version checks (no network access, no prompts).",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AgentsTool {}

// ── GameDay tools ─────────────────────────────────────────────

#[macros::mcp_tool(
    name = "tumult_gameday_run",
    description = "Run a GameDay — execute all experiments in a .gameday.toon file under shared load. Returns resilience score and compliance status.",
    destructive_hint = true,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = true
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayRunTool {
    /// Path to the `.gameday.toon` file.
    pub gameday_path: String,
}

#[macros::mcp_tool(
    name = "tumult_gameday_analyze",
    description = "Analyze a completed GameDay journal — returns resilience score, per-experiment results, and compliance article mapping.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayAnalyzeTool {
    /// Path to the `.gameday.toon` file (reads the .journal.toon alongside it).
    pub gameday_path: String,
}

#[macros::mcp_tool(
    name = "tumult_gameday_create",
    description = "Create a .gameday.toon campaign file (<name>.gameday.toon in the workspace root) from experiment paths, with optional shared load config (load_tool k6 or jmeter, load_script, load_vus) and compliance framework mapping. Fails if the file already exists.",
    destructive_hint = false,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayCreateTool {
    /// `GameDay` name; the file is written as `<name>.gameday.toon`.
    pub name: String,
    /// Experiment `.toon` paths referenced by the campaign (resolved
    /// relative to the gameday file when it is run).
    pub experiments: Vec<String>,
    /// Load tool to run during the campaign: `k6`, `jmeter`, or `none`
    /// (default: no load).
    pub load_tool: Option<String>,
    /// Load script path recorded in the load config.
    pub load_script: Option<String>,
    /// Virtual users for the load test.
    pub load_vus: Option<u32>,
    /// Compliance framework to map: one of `dora`, `nis2`, `pci-dss`,
    /// `iso-22301`, `iso-27001`, `soc2`, `basel-iii`.
    pub framework: Option<String>,
}

#[macros::mcp_tool(
    name = "tumult_gameday_list",
    description = "List available GameDay files (.gameday.toon) in the workspace (sorted by path). Supports limit (default 100, max 1000) and offset; structured content is {items, total, offset, limit}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayListTool {
    /// Optional subdirectory to search within.
    pub path: Option<String>,
    /// Maximum number of entries returned (default 100, max 1000).
    #[serde(default = "default_list_limit")]
    pub limit: u64,
    /// Number of entries to skip before the returned page.
    #[serde(default)]
    pub offset: u64,
}

// ── Intelligence tools (agent reasoning) ─────────────────────

#[macros::mcp_tool(
    name = "tumult_recommend",
    description = "Recommend what to test next — deterministic heuristics over coverage gaps, failure patterns, and stale experiments (shared with `tumult recommend`). Optionally enhance with a local agent CLI adapter (agent=claude-code|codex, plus agent_model, agent_timeout_secs, generate_experiments_dir): this spawns the local agent binary, which may call its model API over the network. Generated experiments pass a parse+validate gate before being written into generate_experiments_dir.",
    destructive_hint = false,
    read_only_hint = false,
    idempotent_hint = false,
    open_world_hint = true
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct RecommendTool {
    #[serde(default = "default_store_path")]
    pub store_path: String,
    pub goal: Option<String>,
    pub model: Option<String>,
    #[serde(default = "default_include_draft")]
    pub include_draft: bool,
    #[serde(default = "default_recommend_format")]
    pub format: String,
    /// Agent CLI adapter name (e.g. `claude-code`, `codex`); enables
    /// agent-enhanced recommendations.
    pub agent: Option<String>,
    /// Model override passed to the agent CLI (requires `agent`).
    pub agent_model: Option<String>,
    /// Agent CLI timeout in seconds (default 120; only used with `agent`).
    #[serde(default = "default_agent_timeout_secs")]
    pub agent_timeout_secs: u64,
    /// Directory (relative to the workspace root) to write validated
    /// agent-proposed experiments into (requires `agent`).
    pub generate_experiments_dir: Option<String>,
}
fn default_include_draft() -> bool {
    true
}
fn default_recommend_format() -> String {
    "text".into()
}
fn default_agent_timeout_secs() -> u64 {
    tumult_agent_cli::adapter::DEFAULT_TIMEOUT.as_secs()
}

#[macros::mcp_tool(
    name = "tumult_coverage",
    description = "Coverage report — which plugins, actions, and targets have been tested vs available. Shows per-plugin test status (FULL/PARTIAL/NONE) and store statistics.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct CoverageTool {
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

// ── ChaosGraph tools ─────────────────────────────────────────

#[macros::mcp_tool(
    name = "tumult_chaosgraph_query",
    description = "ChaosGraph: list graph node ids + one-line summaries for a kind (experiment, fault, service, journal, deviation, compliance_article, coverage_gap, fault_domain) from the persistent analytics store, optionally filtered by a case-insensitive label substring. Small, token-efficient output. Structured content is {kind, count, nodes:[{id,kind,label}]}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ChaosGraphQueryTool {
    /// Node kind: `experiment`, `fault`, `service`, `journal`, `deviation`,
    /// `compliance_article`, `coverage_gap`, or `fault_domain`.
    pub kind: String,
    /// Optional case-insensitive label substring filter.
    pub filter: Option<String>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

#[macros::mcp_tool(
    name = "tumult_chaosgraph_neighbors",
    description = "ChaosGraph: return the ego sub-graph of a node (its neighbourhood within `depth`, default 1) as compact (src)-[rel]->(dst) tuples plus node labels. Optionally filter to a single relation (targets, injects, yielded, observed_on, exhibited, evidences, maps_to_compliance, gap_in). Structured content is {node_id, depth, nodes:[{id,kind,label}], edges:[{src,rel,dst}]}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ChaosGraphNeighborsTool {
    /// The node id to centre on (e.g. `exp:<title>`, `fault:<plugin>::<fn>`).
    pub node_id: String,
    /// Optional relation filter: `targets`, `injects`, `yielded`,
    /// `observed_on`, `exhibited`, `evidences`, `maps_to_compliance`, or
    /// `gap_in`.
    pub rel: Option<String>,
    /// Neighbourhood radius (default 1).
    #[serde(default = "default_graph_depth")]
    pub depth: u32,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}
fn default_graph_depth() -> u32 {
    1
}

#[macros::mcp_tool(
    name = "tumult_chaosgraph_coverage_gaps",
    description = "ChaosGraph: list plugin-catalog actions that have never appeared in a tested run (coverage gaps), optionally filtered by fault domain (plugin substring). When a framework is given (dora, nis2, pci-dss, iso-22301, iso-27001, soc2, basel-iii), also lists that framework's articles still lacking any evidence edge. Refreshes the CoverageGap/FaultDomain nodes + gap_in edges in the store's graph. Structured content is {count, gaps:[{id,plugin,action,domain}], framework?, unevidenced_articles?}.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ChaosGraphCoverageGapsTool {
    /// Optional framework filter: one of `dora`, `nis2`, `pci-dss`,
    /// `iso-22301`, `iso-27001`, `soc2`, `basel-iii`. When set, the response
    /// also lists that framework's still-unevidenced articles.
    pub framework: Option<String>,
    /// Optional fault-domain (plugin) filter — case-insensitive substring of
    /// the plugin name (e.g. `tumult-net`).
    pub domain: Option<String>,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

// ── Agentic AI tools ─────────────────────────────────────────

#[macros::mcp_tool(
    name = "tumult_agentic_list_scenarios",
    description = "List deterministic agentic AI fault-injection scenario packs. Returns metadata only; no prompts or raw payloads.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgenticListScenariosTool {}

#[macros::mcp_tool(
    name = "tumult_agentic_smoke",
    description = "Run a deterministic local agentic AI smoke check with clear fault, contract, expected/actual, and diagnostic feedback. Metadata only; raw payloads are not accepted or returned.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgenticSmokeTool {
    #[serde(default = "default_agentic_adapter")]
    pub adapter: String,
    #[serde(default = "default_agentic_scenario")]
    pub scenario: String,
    pub fault: Option<String>,
    pub contract: Option<String>,
}

#[macros::mcp_tool(
    name = "tumult_agentic_run_experiment",
    description = "Run a deterministic bundled agentic AI experiment with input schema validation. Metadata only; raw payloads are not accepted or returned.",
    read_only_hint = true,
    idempotent_hint = true,
    open_world_hint = false
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgenticRunExperimentTool {
    #[serde(default = "default_agentic_adapter")]
    pub adapter: String,
    #[serde(default = "default_agentic_scenario")]
    pub scenario: String,
    pub fault: Option<String>,
    pub contract: Option<String>,
}
fn default_agentic_adapter() -> String {
    "fake-http".into()
}
fn default_agentic_scenario() -> String {
    "malformed-json-recovery".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_store_path_returns_non_empty_string() {
        // Verifies default_store_path() never silently produces an empty string.
        let path = default_store_path();
        assert!(!path.is_empty(), "default_store_path must not be empty");
    }

    #[test]
    fn recommend_tool_preserves_legacy_store_path_only_args() {
        let args: RecommendTool = serde_json::from_value(serde_json::json!({
            "store_path": "/tmp/tumult.db"
        }))
        .unwrap();

        assert_eq!(args.store_path, "/tmp/tumult.db");
        assert_eq!(args.goal, None);
        assert_eq!(args.model, None);
        assert!(args.include_draft);
        assert_eq!(args.format, "text");
    }

    #[test]
    fn recommend_tool_accepts_expanded_args() {
        let args: RecommendTool = serde_json::from_value(serde_json::json!({
            "store_path": "/tmp/tumult.db",
            "goal": "prioritize payment-path resilience",
            "model": "qwen3",
            "include_draft": false,
            "format": "json"
        }))
        .unwrap();

        assert_eq!(
            args.goal.as_deref(),
            Some("prioritize payment-path resilience")
        );
        assert_eq!(args.model.as_deref(), Some("qwen3"));
        assert!(!args.include_draft);
        assert_eq!(args.format, "json");
    }

    #[test]
    fn recommend_tool_agent_params_default_to_off() {
        let args: RecommendTool = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(args.agent, None);
        assert_eq!(args.agent_model, None);
        assert_eq!(args.agent_timeout_secs, 120);
        assert_eq!(args.generate_experiments_dir, None);

        let args: RecommendTool = serde_json::from_value(serde_json::json!({
            "agent": "claude-code",
            "agent_model": "opus",
            "agent_timeout_secs": 30,
            "generate_experiments_dir": "generated",
        }))
        .unwrap();
        assert_eq!(args.agent.as_deref(), Some("claude-code"));
        assert_eq!(args.agent_model.as_deref(), Some("opus"));
        assert_eq!(args.agent_timeout_secs, 30);
        assert_eq!(args.generate_experiments_dir.as_deref(), Some("generated"));
    }

    #[test]
    fn report_tool_defaults_to_inline_json() {
        let args: ReportTool = serde_json::from_value(serde_json::json!({
            "journal_path": "journal.toon"
        }))
        .unwrap();
        assert_eq!(args.format, "json");
        assert_eq!(args.output_path, None);
    }

    #[test]
    fn trend_tool_defaults_to_resilience_score() {
        let args: TrendTool = serde_json::from_value(serde_json::json!({
            "journals_path": "journals"
        }))
        .unwrap();
        assert_eq!(args.metric, "resilience_score");
        assert_eq!(args.last, None);
        assert_eq!(args.target, None);
    }

    #[test]
    fn gameday_create_tool_requires_only_name_and_experiments() {
        let args: GameDayCreateTool = serde_json::from_value(serde_json::json!({
            "name": "drill",
            "experiments": ["a.toon", "b.toon"],
        }))
        .unwrap();
        assert_eq!(args.name, "drill");
        assert_eq!(args.experiments.len(), 2);
        assert_eq!(args.load_tool, None);
        assert_eq!(args.load_script, None);
        assert_eq!(args.load_vus, None);
        assert_eq!(args.framework, None);
    }

    #[test]
    fn run_experiment_tool_defaults_preserve_legacy_args() {
        // A 2.0.0-era call with only experiment_path must still deserialize,
        // defaulting to JSON output, ingestion enabled, and CLI journal naming.
        let args: RunExperimentTool = serde_json::from_value(serde_json::json!({
            "experiment_path": "exp.toon"
        }))
        .unwrap();
        assert_eq!(args.rollback_strategy, "on-deviation");
        assert_eq!(args.journal_path, None);
        assert!(!args.no_ingest);
        assert_eq!(args.format, "json");
        assert!(!args.store_path.is_empty());
    }

    #[test]
    fn read_journal_tool_defaults_to_full_json() {
        let args: ReadJournalTool = serde_json::from_value(serde_json::json!({
            "journal_path": "journal.toon"
        }))
        .unwrap();
        assert_eq!(args.format, "json");
        assert!(!args.summary);
    }

    #[test]
    fn list_journals_tool_accepts_path_and_legacy_directory_alias() {
        let args: ListJournalsTool =
            serde_json::from_value(serde_json::json!({ "path": "journals" })).unwrap();
        assert_eq!(args.path, "journals");

        // Old clients still send `directory`.
        let args: ListJournalsTool =
            serde_json::from_value(serde_json::json!({ "directory": "journals" })).unwrap();
        assert_eq!(args.path, "journals");
    }

    #[test]
    fn list_tools_pagination_defaults_to_first_hundred() {
        let args: ListJournalsTool =
            serde_json::from_value(serde_json::json!({ "path": "journals" })).unwrap();
        assert_eq!(args.limit, 100);
        assert_eq!(args.offset, 0);

        let args: ListExperimentsTool = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(args.limit, 100);
        assert_eq!(args.offset, 0);

        let args: GameDayListTool = serde_json::from_value(serde_json::json!({
            "limit": 7,
            "offset": 3,
        }))
        .unwrap();
        assert_eq!(args.limit, 7);
        assert_eq!(args.offset, 3);
    }

    #[test]
    fn agentic_smoke_tool_defaults_to_metadata_only_fixture() {
        let args: AgenticSmokeTool = serde_json::from_value(serde_json::json!({})).unwrap();

        assert_eq!(args.adapter, "fake-http");
        assert_eq!(args.scenario, "malformed-json-recovery");
        assert_eq!(args.fault, None);
        assert_eq!(args.contract, None);
    }

    #[test]
    fn agentic_smoke_tool_rejects_raw_payload_fields() {
        let err = serde_json::from_value::<AgenticSmokeTool>(serde_json::json!({
            "scenario": "malformed-json-recovery",
            "prompt": "customer secret"
        }))
        .expect_err("raw prompt fields must not be accepted");

        assert!(
            err.to_string().contains("unknown field"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn agentic_run_tool_defaults_and_rejects_raw_payload_fields() {
        let args: AgenticRunExperimentTool = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(args.adapter, "fake-http");
        assert_eq!(args.scenario, "malformed-json-recovery");

        let err = serde_json::from_value::<AgenticRunExperimentTool>(serde_json::json!({
            "scenario": "malformed-json-recovery",
            "completion": "raw model output"
        }))
        .expect_err("raw completion fields must not be accepted");

        assert!(
            err.to_string().contains("unknown field"),
            "unexpected error: {err}"
        );
    }
}
