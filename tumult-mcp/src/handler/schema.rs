//! MCP tool schema definitions and their default-value helpers.

use rust_mcp_sdk::macros;

// ── Tool schema definitions ───────────────────────────────────

#[macros::mcp_tool(
    name = "tumult_run_experiment",
    description = "Execute a Tumult chaos experiment and return the journal."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct RunExperimentTool {
    pub experiment_path: String,
    #[serde(default = "default_strategy")]
    pub rollback_strategy: String,
}
fn default_strategy() -> String {
    "on-deviation".into()
}

#[macros::mcp_tool(
    name = "tumult_validate",
    description = "Validate an experiment file for syntax and provider support."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ValidateTool {
    pub experiment_path: String,
}

#[macros::mcp_tool(
    name = "tumult_analyze",
    description = "SQL query over experiment journals via embedded DuckDB."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AnalyzeTool {
    pub journals_path: String,
    pub query: String,
}

#[macros::mcp_tool(
    name = "tumult_read_journal",
    description = "Read a TOON journal file and return its contents."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ReadJournalTool {
    pub journal_path: String,
}

#[macros::mcp_tool(
    name = "tumult_list_journals",
    description = "List .toon journal files in a directory."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ListJournalsTool {
    pub directory: String,
}

#[macros::mcp_tool(
    name = "tumult_discover",
    description = "List all Tumult plugins, actions, and probes."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct DiscoverTool {}

#[macros::mcp_tool(
    name = "tumult_create_experiment",
    description = "Create a new experiment from a template."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct CreateExperimentTool {
    pub output_path: String,
    pub plugin: Option<String>,
}

#[macros::mcp_tool(
    name = "tumult_query_traces",
    description = "Query trace data from a journal — returns activity spans with trace/span IDs for observability correlation."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct QueryTracesTool {
    pub journal_path: String,
}

#[macros::mcp_tool(
    name = "tumult_store_stats",
    description = "Get persistent analytics store statistics — experiment count, activity count, schema version, file size."
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
    description = "SQL query over the persistent analytics store (accumulated history from all runs)."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct AnalyzeStoreTool {
    pub query: String,
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

#[macros::mcp_tool(
    name = "tumult_list_experiments",
    description = "List all .toon experiment files recursively from the workspace or a given path."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct ListExperimentsTool {
    /// Optional subdirectory to search within (relative to workspace root).
    pub path: Option<String>,
}

// ── GameDay tools ─────────────────────────────────────────────

#[macros::mcp_tool(
    name = "tumult_gameday_run",
    description = "Run a GameDay — execute all experiments in a .gameday.toon file under shared load. Returns resilience score and compliance status."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayRunTool {
    /// Path to the `.gameday.toon` file.
    pub gameday_path: String,
}

#[macros::mcp_tool(
    name = "tumult_gameday_analyze",
    description = "Analyze a completed GameDay journal — returns resilience score, per-experiment results, and compliance article mapping."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayAnalyzeTool {
    /// Path to the `.gameday.toon` file (reads the .journal.toon alongside it).
    pub gameday_path: String,
}

#[macros::mcp_tool(
    name = "tumult_gameday_list",
    description = "List available GameDay files (.gameday.toon) in the workspace."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct GameDayListTool {
    /// Optional subdirectory to search within.
    pub path: Option<String>,
}

// ── Intelligence tools (agent reasoning) ─────────────────────

#[macros::mcp_tool(
    name = "tumult_recommend",
    description = "Recommend what to test next — analyzes coverage gaps, failure patterns, and stale experiments. Returns actionable suggestions for an agent or engineer."
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
}
fn default_include_draft() -> bool {
    true
}
fn default_recommend_format() -> String {
    "text".into()
}

#[macros::mcp_tool(
    name = "tumult_coverage",
    description = "Coverage report — which plugins, actions, and targets have been tested vs available. Shows per-plugin test status (FULL/PARTIAL/NONE) and store statistics."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
pub struct CoverageTool {
    #[serde(default = "default_store_path")]
    pub store_path: String,
}

// ── Agentic AI tools ─────────────────────────────────────────

#[macros::mcp_tool(
    name = "tumult_agentic_list_scenarios",
    description = "List deterministic agentic AI fault-injection scenario packs. Returns metadata only; no prompts or raw payloads."
)]
#[derive(Debug, serde::Deserialize, serde::Serialize, macros::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgenticListScenariosTool {}

#[macros::mcp_tool(
    name = "tumult_agentic_smoke",
    description = "Run a deterministic local agentic AI smoke check with clear fault, contract, expected/actual, and diagnostic feedback. Metadata only; raw payloads are not accepted or returned."
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
    description = "Run a deterministic bundled agentic AI experiment with input schema validation. Metadata only; raw payloads are not accepted or returned."
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
