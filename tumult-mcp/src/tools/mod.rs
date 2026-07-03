//! Tool implementations for the Tumult MCP server.
//!
//! Each function handles a single MCP tool call and returns
//! structured text content. The implementations are split into cohesive
//! submodules and re-exported here to preserve the flat `tools::*` API.

mod agentic;
mod analysis;
mod experiment;
mod gameday;
mod journals;
mod listing;
mod recommend;
mod validation;

pub use agentic::{agentic_list_scenarios, agentic_run_experiment, agentic_smoke};
pub use analysis::{analyze, analyze_persistent, store_stats};
pub use experiment::{create_experiment, run_experiment, validate_experiment};
pub use gameday::{gameday_analyze, gameday_list, gameday_run};
pub use journals::{discover_plugins, list_journals, query_traces, read_journal};
pub use listing::list_experiments;
pub use recommend::{coverage, recommend};
pub use validation::{safe_resolve_path, validate_action_name, validate_select_only};

#[cfg(test)]
mod test_support {
    /// Write a minimal valid experiment TOON file into `dir` and return its path.
    pub(crate) fn write_valid_experiment(dir: &std::path::Path) -> String {
        let exp = tumult_core::types::Experiment {
            title: "MCP test experiment".into(),
            method: vec![tumult_core::types::Activity {
                name: "echo-action".into(),
                activity_type: tumult_core::types::ActivityType::Action,
                provider: tumult_core::types::Provider::Process {
                    path: "echo".into(),
                    arguments: vec!["hello".into()],
                    env: std::collections::HashMap::new(),
                    timeout_s: Some(5.0),
                },
                tolerance: None,
                pause_before_s: None,
                pause_after_s: None,
                background: false,
                label_selector: None,
            }],
            ..Default::default()
        };
        let toon = toon_format::encode_default(&exp).unwrap();
        let path = dir.join("test.toon");
        std::fs::write(&path, toon).unwrap();
        path.to_str().unwrap().to_string()
    }
}
