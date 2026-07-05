//! MCP tool schema definitions and their default-value helpers.

mod agentic;
mod authoring;
mod chaosgraph;
mod core_tools;
mod gameday;
mod intelligence;

pub use agentic::*;
pub use authoring::*;
pub use chaosgraph::*;
pub use core_tools::*;
pub use gameday::*;
pub use intelligence::*;

/// Default page size for the paginating list tools (journals, experiments,
/// gamedays).
pub(crate) fn default_list_limit() -> u64 {
    100
}

/// Default persistent analytics store path, shared by every tool that reads
/// or writes the store.
pub(crate) fn default_store_path() -> String {
    let path = tumult_analytics::AnalyticsStore::default_path();
    path.to_str().map_or_else(
        || ".tumult/analytics.db".to_string(),
        std::string::ToString::to_string,
    )
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
