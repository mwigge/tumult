//! Tool implementations for the Tumult MCP server.
//!
//! Each function handles a single MCP tool call and returns
//! structured text content. The implementations are split into cohesive
//! submodules and re-exported here to preserve the flat `tools::*` API.

mod agentic;
mod agents;
mod analysis;
mod experiment;
mod gameday;
mod graph;
mod journals;
mod listing;
mod recommend;
mod reporting;
mod validation;

pub use agentic::{agentic_list_scenarios, agentic_run_experiment, agentic_smoke};
pub use agents::agents;
pub use analysis::{analyze, analyze_persistent, store_stats};
pub use experiment::{
    create_experiment, run_experiment, validate_experiment, RunExperimentRequest,
};
pub use gameday::{
    gameday_analyze, gameday_create, gameday_list, gameday_run, GameDayCreateRequest,
};
pub use graph::{chaosgraph_coverage_gaps, chaosgraph_neighbors, chaosgraph_query};
pub use journals::{discover_plugins, list_journals, query_traces, read_journal};
pub(crate) use listing::extract_title;
pub use listing::list_experiments;
pub use recommend::{coverage, recommend, RecommendRequest};
pub use reporting::{compliance, report, trend};
pub use validation::{
    safe_resolve_output_path, safe_resolve_path, validate_action_name, validate_select_only,
};

/// A tool result carrying both human-readable text content and the
/// structured JSON object placed in `CallToolResult::structured_content`.
///
/// The structured map is the source of truth; `text` is a rendering of it
/// (or a raw/TOON rendering) and may be truncated to [`MAX_TEXT_BYTES`].
#[derive(Debug)]
pub struct StructuredReport {
    /// Text content returned to the client (possibly truncated).
    pub text: String,
    /// Structured content object; must conform to the tool's advertised
    /// output schema.
    pub structured: serde_json::Map<String, serde_json::Value>,
}

/// Maximum size of the text content returned by journal-bearing tools.
pub(crate) const MAX_TEXT_BYTES: usize = 512 * 1024;

/// Cap `text` at [`MAX_TEXT_BYTES`], appending an explicit truncation notice
/// (including `hint` if non-empty). Truncation happens on a char boundary.
pub(crate) fn cap_text(text: String, hint: &str) -> String {
    use std::fmt::Write as _;

    if text.len() <= MAX_TEXT_BYTES {
        return text;
    }
    let total = text.len();
    let mut cut = MAX_TEXT_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut capped = text[..cut].to_string();
    let _ = write!(capped, "\n[truncated: showing first {cut} of {total} bytes");
    if !hint.is_empty() {
        capped.push_str("; ");
        capped.push_str(hint);
    }
    capped.push(']');
    capped
}

#[cfg(test)]
mod cap_text_tests {
    use super::{cap_text, MAX_TEXT_BYTES};

    #[test]
    fn cap_text_leaves_small_text_untouched() {
        let text = "hello".to_string();
        assert_eq!(cap_text(text.clone(), "hint"), text);
    }

    #[test]
    fn cap_text_truncates_and_notes() {
        let text = "x".repeat(MAX_TEXT_BYTES + 100);
        let capped = cap_text(text, "use summary=true");
        assert!(capped.len() < MAX_TEXT_BYTES + 100);
        assert!(capped.contains("[truncated: showing first"));
        assert!(capped.contains("use summary=true"));
    }

    #[test]
    fn cap_text_respects_char_boundaries() {
        // Multi-byte chars around the cut point must not split.
        let text = "é".repeat(MAX_TEXT_BYTES); // 2 bytes each
        let capped = cap_text(text, "");
        assert!(capped.contains("[truncated"));
        // Must be valid UTF-8 by construction; also check no panic occurred
        // and the notice does not carry a dangling hint separator.
        assert!(!capped.contains("; ]"));
    }
}

#[cfg(test)]
pub(crate) mod test_support {
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

    /// Run the minimal valid experiment and write its journal into `dir`.
    /// Returns the journal path. Ingestion is skipped.
    pub(crate) fn write_run_journal(dir: &std::path::Path) -> std::path::PathBuf {
        let exp_path = write_valid_experiment(dir);
        let journal_path = dir.join("journal.toon");
        crate::tools::run_experiment(crate::tools::RunExperimentRequest {
            experiment_path: &exp_path,
            rollback_strategy: "always",
            journal_path: &journal_path,
            store_path: "unused.duckdb",
            no_ingest: true,
            format: "toon",
            parent_context: None,
        })
        .unwrap();
        journal_path
    }
}
