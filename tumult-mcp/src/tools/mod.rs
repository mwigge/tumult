//! Tool implementations for the Tumult MCP server.
//!
//! Each function handles a single MCP tool call and returns
//! structured text content. The implementations are split into cohesive
//! submodules and re-exported here to preserve the flat `tools::*` API.

mod agentic;
mod agents;
mod analysis;
mod authoring;
/// Autopilot tools: decision-loop passes, status, human responses, export,
/// and change notifications.
pub mod autopilot;
mod cypher;
mod experiment;
mod gameday;
mod graph;
mod journals;
mod listing;
mod recommend;
mod reporting;
/// Topology tools: declared-topology import, service map, compliance
/// lineage, and injection recommendations over the analytics store.
pub(crate) mod topology;
mod validation;
mod whoami;

pub use agentic::{agentic_list_scenarios, agentic_run_experiment, agentic_smoke};
pub use agents::agents;
pub use analysis::{analyze, analyze_persistent, store_stats};
pub use authoring::{fault_catalog, scaffold_experiment, ScaffoldArgs};
pub use autopilot::{
    autopilot_export, autopilot_notify_change, autopilot_once, autopilot_respond, autopilot_status,
};
pub use cypher::chaosgraph_cypher;
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
pub use topology::{compliance_lineage, recommend_injection, topology_import, topology_map};
pub use validation::{
    safe_resolve_output_path, safe_resolve_path, validate_action_name, validate_select_only,
};
pub use whoami::whoami;

/// A tool result carrying both human-readable text content and the
/// structured JSON object placed in `CallToolResult::structured_content`.
///
/// The structured map is the source of truth; `text` is a rendering of it
/// (or a raw/TOON rendering) and may be truncated to the server's text limit.
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
    const PLUGIN_PATH_ENV: &str = "TUMULT_PLUGIN_PATH";

    /// Point plugin discovery at `path` for the life of the returned guard.
    ///
    /// The guard is the process-wide environment lock, so a test that changes
    /// more variables does it through the returned guard instead of taking a
    /// second one.
    pub(crate) fn plugin_path_guard(path: &std::path::Path) -> tumult_test_utils::EnvGuard {
        let mut env = tumult_test_utils::EnvGuard::lock();
        env.set(PLUGIN_PATH_ENV, path);
        env
    }

    /// Point plugin discovery at a catalog written into `dir`, one script
    /// plugin per `(name, actions)`, and at an empty home, for the life of
    /// the returned guard.
    pub(crate) fn catalog_guard(
        dir: &std::path::Path,
        plugins: &[(&str, &[&str])],
    ) -> tumult_test_utils::EnvGuard {
        let catalog = dir.join("catalog");
        for (name, actions) in plugins {
            let manifest = tumult_plugin::manifest::ScriptPluginManifest {
                name: (*name).into(),
                version: "0.1.0".into(),
                description: "test catalog".into(),
                actions: actions
                    .iter()
                    .map(|action| tumult_plugin::manifest::ScriptAction {
                        name: (*action).into(),
                        script: format!("actions/{action}.sh").into(),
                        description: "test action".into(),
                    })
                    .collect(),
                probes: vec![],
            };
            let plugin_dir = catalog.join(name);
            std::fs::create_dir_all(&plugin_dir).unwrap();
            let toon = toon_format::encode_default(&manifest).unwrap();
            std::fs::write(plugin_dir.join("plugin.toon"), toon).unwrap();
        }
        let mut env = plugin_path_guard(&catalog);
        env.set("HOME", dir.join("home"));
        env
    }

    /// An action named `name` that calls `function` of the script plugin
    /// `plugin`.
    pub(crate) fn script_action(
        name: &str,
        plugin: &str,
        function: &str,
    ) -> tumult_core::types::Activity {
        tumult_core::types::Activity {
            name: name.into(),
            activity_type: tumult_core::types::ActivityType::Action,
            provider: tumult_core::types::Provider::Script {
                plugin: plugin.into(),
                function: function.into(),
                arguments: std::collections::HashMap::new(),
                timeout_s: None,
            },
            ..Default::default()
        }
    }

    /// The journal of run `id` of `title`, whose method ran the actions
    /// `method` and whose rollbacks ran the actions `rollbacks`, by
    /// activity name.
    pub(crate) fn journal_of(
        id: &str,
        title: &str,
        status: tumult_core::types::ExperimentStatus,
        method: &[&str],
        rollbacks: &[&str],
    ) -> tumult_core::types::Journal {
        use tumult_core::types::{ActivityResult, ActivityStatus, ActivityType, SpanId, TraceId};
        let ran = |names: &[&str]| -> Vec<ActivityResult> {
            names
                .iter()
                .map(|name| ActivityResult {
                    name: (*name).into(),
                    activity_type: ActivityType::Action,
                    status: ActivityStatus::Succeeded,
                    started_at_ns: 1,
                    duration_ms: 1,
                    output: None,
                    error: None,
                    trace_id: TraceId::empty(),
                    span_id: SpanId::empty(),
                })
                .collect()
        };
        let mut journal: tumult_core::types::Journal = serde_json::from_value(serde_json::json!({
            "experiment_title": title,
            "experiment_id": id,
            "status": "completed",
            "started_at_ns": 1,
            "ended_at_ns": 2,
            "duration_ms": 1,
            "method_results": [],
            "rollback_results": [],
        }))
        .unwrap();
        journal.status = status;
        journal.method_results = ran(method);
        journal.rollback_results = ran(rollbacks);
        journal
    }

    /// The workspace's real `plugins/` directory (the live fault catalog).
    pub(crate) fn workspace_plugins_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../plugins")
            .canonicalize()
            .unwrap()
    }

    /// Write a minimal valid experiment TOON file into `dir` and return its path.
    pub(crate) fn write_valid_experiment(dir: &std::path::Path) -> String {
        write_experiment(dir, &valid_experiment())
    }

    /// Write `experiment` as `test.toon` into `dir` and return its path.
    fn write_experiment(
        dir: &std::path::Path,
        experiment: &tumult_core::types::Experiment,
    ) -> String {
        let toon = toon_format::encode_default(experiment).unwrap();
        let path = dir.join("test.toon");
        std::fs::write(&path, toon).unwrap();
        path.to_str().unwrap().to_string()
    }

    /// A minimal valid experiment: one action that echoes.
    fn valid_experiment() -> tumult_core::types::Experiment {
        tumult_core::types::Experiment {
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
        }
    }

    /// Run the minimal valid experiment and write its journal into `dir`.
    /// Returns the journal path. Ingestion is skipped.
    pub(crate) fn write_run_journal(dir: &std::path::Path) -> std::path::PathBuf {
        let exp_path = write_valid_experiment(dir);
        let journal_path = dir.join("journal.toon");
        run_to_journal(&exp_path, &journal_path);
        journal_path
    }

    /// Run the minimal valid experiment, named `title`, on the target
    /// `system` of the technology `technology`, and write its journal into
    /// `journals` as `<system>.toon`. Returns the journal path. The
    /// definition is kept out of `journals`, which holds journals only.
    pub(crate) fn write_run_journal_on(
        journals: &std::path::Path,
        title: &str,
        system: &str,
        technology: &str,
    ) -> std::path::PathBuf {
        let definitions = tempfile::TempDir::new().unwrap();
        let experiment = tumult_core::types::Experiment {
            title: title.into(),
            target: Some(tumult_core::types::ExperimentTarget {
                system: system.into(),
                technology: Some(technology.into()),
                environment: None,
                criticality: None,
                namespace: None,
                version: None,
            }),
            ..valid_experiment()
        };
        let exp_path = write_experiment(definitions.path(), &experiment);
        let journal_path = journals.join(format!("{system}.toon"));
        run_to_journal(&exp_path, &journal_path);
        journal_path
    }

    /// Run the experiment at `exp_path` and write its journal to
    /// `journal_path`. Ingestion is skipped.
    fn run_to_journal(exp_path: &str, journal_path: &std::path::Path) {
        crate::tools::run_experiment(crate::tools::RunExperimentRequest {
            experiment_path: exp_path,
            rollback_strategy: "always",
            journal_path,
            store_path: "unused.duckdb",
            no_ingest: true,
            format: "toon",
            parent_context: None,
        })
        .unwrap();
    }
}
