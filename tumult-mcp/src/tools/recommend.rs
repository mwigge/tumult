//! Advisory tools: `recommend` next tests and `coverage` reporting.
//!
//! `recommend` is a thin adapter over `tumult-intelligence` — the same
//! heuristic pipeline, agent enhancement, and experiment validation gate the
//! CLI uses — so the two surfaces cannot drift apart.

use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

use crate::error::ToolError;
use crate::tools::validation::validate_action_name;
use crate::tools::StructuredReport;

use tumult_intelligence::{AgentOptions, OutputFormat, RecommendOptions};

/// Parameters for [`recommend`].
pub struct RecommendRequest<'a> {
    /// Analytics store the heuristics run over.
    pub store_path: &'a str,
    /// Operator goal woven into the recommendations.
    pub goal: Option<&'a str>,
    /// Model label recorded in the deterministic metadata.
    pub model: Option<&'a str>,
    /// Whether to include a draft TOON experiment when one is proposed.
    pub include_draft: bool,
    /// Text rendering: `text` or `json`.
    pub format: &'a str,
    /// Agent CLI adapter name (e.g. `claude-code`); enables enhancement.
    pub agent: Option<&'a str>,
    /// Model override passed to the agent CLI (requires `agent`).
    pub agent_model: Option<&'a str>,
    /// Agent CLI timeout in seconds.
    pub agent_timeout_secs: u64,
    /// Directory for validated agent-proposed experiments (already
    /// resolved/contained by the caller; requires `agent`).
    pub generate_dir: Option<&'a Path>,
    /// Workspace root used as the agent subprocess working directory.
    pub workspace_root: &'a Path,
}

/// Returns recommendations for what to test next via `tumult-intelligence`
/// (heuristics plus optional agent-CLI enhancement).
///
/// The structured object carries the serialized `RecommendationOutput`
/// (or a `message` when no store exists), plus an `agent` object when an
/// adapter ran; `format` selects the text rendering (`text` or `json`).
///
/// # Errors
///
/// Returns a [`ToolError`] for an invalid format, agent parameters without
/// `agent`, an unknown adapter (listing the available ones), agent CLI
/// failures, or unwritable experiment output.
pub fn recommend(request: &RecommendRequest<'_>) -> Result<StructuredReport, ToolError> {
    let format = match request.format {
        "text" => OutputFormat::Text,
        "json" => OutputFormat::Json,
        other => {
            return Err(ToolError::InvalidInput(format!(
                "unsupported recommend format '{other}'; expected text or json"
            )))
        }
    };
    if request.agent.is_none() && (request.agent_model.is_some() || request.generate_dir.is_some())
    {
        return Err(ToolError::InvalidInput(
            "agent_model and generate_experiments_dir require the agent parameter".into(),
        ));
    }

    // Validate the adapter name up front (before the store-missing early
    // return) so a typo'd adapter is reported even without run history.
    let registry = tumult_agent_cli::AdapterRegistry::builtin();
    let adapter = request
        .agent
        .map(|name| registry.get(name))
        .transpose()
        .map_err(|e| ToolError::InvalidInput(e.to_string()))?;

    let store_path = std::path::PathBuf::from(request.store_path);
    if !store_path.exists() {
        let message = "No analytics store found. Run some experiments first.".to_string();
        let mut structured = serde_json::Map::new();
        structured.insert("message".into(), serde_json::json!(message));
        return Ok(StructuredReport {
            text: message,
            structured,
        });
    }

    let options = RecommendOptions {
        store_path,
        goal: request.goal.map(str::to_string),
        model: request.model.map(str::to_string),
        include_draft: request.include_draft,
        format,
    };
    let heuristic = tumult_intelligence::recommend_output(&options);
    let base_text = tumult_intelligence::render(&heuristic, format)
        .map_err(|e| ToolError::Execution(e.to_string()))?;

    let Some(adapter) = adapter else {
        let value =
            serde_json::to_value(&heuristic).map_err(|e| ToolError::Execution(e.to_string()))?;
        let serde_json::Value::Object(structured) = value else {
            unreachable!("RecommendationOutput serializes as a JSON object");
        };
        return Ok(StructuredReport {
            text: crate::tools::cap_text(base_text, ""),
            structured,
        });
    };

    let agent_options = AgentOptions {
        model: request.agent_model.map(str::to_string),
        timeout: Duration::from_secs(request.agent_timeout_secs),
        generate_experiments: request.generate_dir.is_some(),
        workspace: request.workspace_root.to_path_buf(),
    };
    let enhancement = tumult_intelligence::enhance(&heuristic, adapter, &agent_options)
        .map_err(|e| ToolError::Execution(e.to_string()))?;

    let outcome = match request.generate_dir {
        Some(dir) => {
            tumult_intelligence::write_validated_experiments(dir, &enhancement.experiments)
                .map_err(|e| ToolError::Execution(e.to_string()))?
        }
        None => tumult_intelligence::WriteOutcome::default(),
    };

    let value = tumult_intelligence::json_with_agent(&heuristic, &enhancement, &outcome)
        .map_err(|e| ToolError::Execution(e.to_string()))?;
    let serde_json::Value::Object(structured) = value else {
        unreachable!("RecommendationOutput serializes as a JSON object");
    };

    let text = match format {
        OutputFormat::Text => tumult_intelligence::render_text_with_agent(
            &base_text,
            &enhancement,
            request.generate_dir.is_some().then_some(&outcome),
        ),
        OutputFormat::Json => {
            serde_json::to_string_pretty(&serde_json::Value::Object(structured.clone()))
                .map_err(|e| ToolError::Execution(e.to_string()))?
        }
    };

    Ok(StructuredReport {
        text: crate::tools::cap_text(text, ""),
        structured,
    })
}

/// Returns a coverage report — which plugins, targets, and fault types
/// have been tested vs what is available.
///
/// The structured object contains `plugins` (per-plugin coverage entries)
/// and `store` (summary counts, or `null` when no analytics store exists).
///
/// # Errors
///
/// Returns a [`ToolError`] if the store cannot be opened or queried.
pub fn coverage(store_path: &str) -> Result<StructuredReport, ToolError> {
    let path = std::path::PathBuf::from(store_path);

    // Available capabilities
    let available_plugins = tumult_plugin::discovery::discover_all_plugins().unwrap_or_default();
    let mut output = String::new();

    writeln!(output, "=== Coverage Report ===").ok();
    writeln!(output).ok();

    // Plugin-level coverage
    writeln!(output, "Plugin coverage:").ok();

    let store = if path.exists() {
        tumult_lake::AnalyticsStore::open_read_only(&path).ok()
    } else {
        None
    };

    let mut plugin_entries: Vec<serde_json::Value> = Vec::with_capacity(available_plugins.len());
    for plugin in &available_plugins {
        let action_count = plugin.actions.len();
        let probe_count = plugin.probes.len();

        let tested_count = if let Some(ref s) = store {
            // Count distinct action names from this plugin that appear in results
            let action_names: Vec<String> = plugin.actions.iter().map(|a| a.name.clone()).collect();
            let mut count = 0;
            for name in &action_names {
                // Validate the name before interpolating into a query to
                // prevent SQL injection via a crafted plugin manifest.
                if validate_action_name(name).is_err() {
                    continue;
                }
                let q =
                    format!("SELECT count(*) FROM activity_results WHERE name = '{name}' LIMIT 1");
                if let Ok(rows) = s.query(&q) {
                    if let Some(row) = rows.first() {
                        if let Some(val) = row.first() {
                            if val != "0" {
                                count += 1;
                            }
                        }
                    }
                }
            }
            count
        } else {
            0
        };

        let status = if tested_count == action_count && action_count > 0 {
            "FULL"
        } else if tested_count > 0 {
            "PARTIAL"
        } else {
            "NONE"
        };

        writeln!(
            output,
            "  {:<25} {tested_count}/{action_count} actions tested, {probe_count} probes  [{status}]",
            plugin.name
        )
        .ok();
        plugin_entries.push(serde_json::json!({
            "name": plugin.name,
            "actions_total": action_count,
            "actions_tested": tested_count,
            "probes": probe_count,
            "status": status,
        }));
    }

    // Summary stats from store
    let store_summary = if let Some(ref s) = store {
        writeln!(output).ok();
        writeln!(output, "Store summary:").ok();

        let stats = s.stats().map_err(|e| ToolError::Store(e.to_string()))?;
        let pass_count: u64 = s
            .query("SELECT count(*) FROM experiments WHERE status = 'completed'")
            .ok()
            .and_then(|r| r.first().cloned())
            .and_then(|r| r.first().cloned())
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        writeln!(output, "  Experiments: {}", stats.experiment_count).ok();
        writeln!(output, "  Activities: {}", stats.activity_count).ok();
        writeln!(
            output,
            "  Pass rate: {pass_count}/{}",
            stats.experiment_count
        )
        .ok();

        // Distinct targets
        let targets = s
            .query("SELECT DISTINCT title FROM experiments ORDER BY title")
            .unwrap_or_default();
        writeln!(output, "  Distinct experiment types: {}", targets.len()).ok();

        serde_json::json!({
            "experiments": stats.experiment_count,
            "activities": stats.activity_count,
            "passed": pass_count,
            "distinct_experiment_types": targets.len(),
        })
    } else {
        writeln!(output).ok();
        writeln!(
            output,
            "No analytics store found. Run experiments to build coverage data."
        )
        .ok();
        serde_json::Value::Null
    };

    let mut structured = serde_json::Map::new();
    structured.insert("plugins".into(), serde_json::Value::Array(plugin_entries));
    structured.insert("store".into(), store_summary);

    Ok(StructuredReport {
        text: output,
        structured,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request<'a>(store_path: &'a str, format: &'a str) -> RecommendRequest<'a> {
        RecommendRequest {
            store_path,
            goal: None,
            model: None,
            include_draft: false,
            format,
            agent: None,
            agent_model: None,
            agent_timeout_secs: 30,
            generate_dir: None,
            workspace_root: Path::new("."),
        }
    }

    /// Create an empty analytics store and return its path.
    fn empty_store(dir: &Path) -> std::path::PathBuf {
        let db = dir.join("analytics.duckdb");
        drop(tumult_lake::AnalyticsStore::open(&db).unwrap());
        db
    }

    #[test]
    fn recommend_rejects_an_unknown_format() {
        let err = recommend(&request("unused.duckdb", "yaml"))
            .expect_err("unsupported formats must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("yaml"), "must name the bad value: {msg}");
        assert!(msg.contains("text") && msg.contains("json"), "got: {msg}");
    }

    #[test]
    fn recommend_requires_agent_for_agent_only_parameters() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut req = request("unused.duckdb", "text");
        req.agent_model = Some("claude-opus");
        let err = recommend(&req).expect_err("agent_model without agent must be rejected");
        assert!(err.to_string().contains("agent"), "got: {err}");

        req.agent_model = None;
        req.generate_dir = Some(dir.path());
        let err = recommend(&req).expect_err("generate_dir without agent must be rejected");
        assert!(err.to_string().contains("agent"), "got: {err}");
    }

    #[test]
    fn recommend_rejects_an_unknown_adapter_before_touching_the_store() {
        let mut req = request("unused.duckdb", "text");
        req.agent = Some("no-such-adapter");
        let err = recommend(&req).expect_err("an unknown adapter must be rejected");
        assert!(err.to_string().contains("no-such-adapter"), "got: {err}");
    }

    #[test]
    fn recommend_reports_when_no_store_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("no-store.duckdb");
        let report = recommend(&request(missing.to_str().unwrap(), "text")).unwrap();
        assert_eq!(
            report.structured["message"],
            "No analytics store found. Run some experiments first."
        );
        assert_eq!(report.text, report.structured["message"]);
    }

    #[test]
    fn recommend_returns_heuristic_output_over_an_empty_store() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = empty_store(dir.path());

        let mut req = request(db.to_str().unwrap(), "text");
        req.goal = Some("cover the data tier");
        let report = recommend(&req).unwrap();
        assert_eq!(report.structured["source"], "heuristic-fallback");
        assert_eq!(report.structured["goal"], "cover the data tier");
        assert!(
            report.structured["recommendations"]
                .as_array()
                .is_some_and(|r| !r.is_empty()),
            "heuristics always produce at least one recommendation: {:?}",
            report.structured
        );
        assert!(!report.text.is_empty());

        // The json format renders the same structured object as pretty JSON.
        req.format = "json";
        let report = recommend(&req).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&report.text).unwrap();
        assert_eq!(parsed["source"], "heuristic-fallback");
    }

    #[test]
    fn recommend_can_include_a_draft_experiment() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = empty_store(dir.path());
        let mut req = request(db.to_str().unwrap(), "text");
        req.include_draft = true;
        let report = recommend(&req).unwrap();
        // The draft is validated before being attached; the flag records the outcome.
        assert!(report.structured.get("draft_valid").is_some());
    }

    #[test]
    fn coverage_without_a_store_reports_no_store() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("no-store.duckdb");
        let report = coverage(missing.to_str().unwrap()).unwrap();
        assert!(report.structured["store"].is_null());
        assert!(report.structured["plugins"].as_array().is_some());
        assert!(
            report.text.contains("No analytics store found"),
            "{}",
            report.text
        );
    }

    #[test]
    fn coverage_over_an_empty_store_reports_zero_counts() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = empty_store(dir.path());
        let report = coverage(db.to_str().unwrap()).unwrap();

        let store = &report.structured["store"];
        assert_eq!(store["experiments"], 0);
        assert_eq!(store["activities"], 0);
        assert_eq!(store["passed"], 0);
        assert_eq!(store["distinct_experiment_types"], 0);
        assert!(report.text.contains("Store summary:"), "{}", report.text);
        assert!(report.text.contains("Experiments: 0"), "{}", report.text);
        assert!(report.text.contains("Pass rate: 0/0"), "{}", report.text);
        // Every discovered plugin is untested against an empty store.
        for entry in report.structured["plugins"].as_array().unwrap() {
            assert_eq!(entry["actions_tested"], 0);
            assert_eq!(entry["status"], "NONE");
        }
    }

    #[test]
    fn coverage_counts_tested_actions_from_the_store() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = empty_store(dir.path());
        // Point discovery at a temp catalog with one plugin and two actions.
        let plugin_dir = dir.path().join("catalog").join("cov-plugin");
        std::fs::create_dir_all(&plugin_dir).unwrap();
        let manifest = tumult_plugin::manifest::ScriptPluginManifest {
            name: "cov-plugin".into(),
            version: "0.1.0".into(),
            description: "coverage test".into(),
            actions: vec![
                tumult_plugin::manifest::ScriptAction {
                    name: "cov-action-tested".into(),
                    script: "actions/tested.sh".into(),
                    description: "tested action".into(),
                },
                tumult_plugin::manifest::ScriptAction {
                    name: "cov-action-untested".into(),
                    script: "actions/untested.sh".into(),
                    description: "untested action".into(),
                },
            ],
            probes: vec![],
        };
        let toon = toon_format::encode_default(&manifest).unwrap();
        std::fs::write(plugin_dir.join("plugin.toon"), toon).unwrap();
        std::env::set_var("TUMULT_PLUGIN_PATH", dir.path().join("catalog"));

        // A completed run that exercised exactly one of the two actions.
        let journal = tumult_core::types::Journal {
            experiment_title: "coverage drill".into(),
            experiment_id: "run-cov-1".into(),
            status: tumult_core::types::ExperimentStatus::Completed,
            started_at_ns: 1,
            ended_at_ns: 2,
            duration_ms: 1,
            steady_state_before: None,
            steady_state_after: None,
            method_results: vec![tumult_core::types::ActivityResult {
                name: "cov-action-tested".into(),
                activity_type: tumult_core::types::ActivityType::Action,
                status: tumult_core::types::ActivityStatus::Succeeded,
                started_at_ns: 1,
                duration_ms: 1,
                output: None,
                error: None,
                trace_id: tumult_core::types::TraceId::empty(),
                span_id: tumult_core::types::SpanId::empty(),
            }],
            rollback_results: vec![],
            rollback_failures: 0,
            estimate: None,
            baseline_result: None,
            during_result: None,
            post_result: None,
            load_result: None,
            analysis: None,
            regulatory: None,
            halt: None,
            blast_radius: None,
        };
        let store = tumult_lake::AnalyticsStore::open(&db).unwrap();
        store
            .ingest_journal_with_experiment(&journal, None)
            .unwrap();
        drop(store);

        let report = coverage(db.to_str().unwrap()).unwrap();
        let plugins = report.structured["plugins"].as_array().unwrap();
        let entry = plugins
            .iter()
            .find(|p| p["name"] == "cov-plugin")
            .expect("the catalog plugin must be discovered");
        assert_eq!(entry["actions_total"], 2);
        assert_eq!(entry["actions_tested"], 1);
        assert_eq!(entry["status"], "PARTIAL");
        assert!(
            report.text.contains("1/2 actions tested"),
            "{}",
            report.text
        );

        let store_summary = &report.structured["store"];
        assert_eq!(store_summary["experiments"], 1);
        assert_eq!(store_summary["passed"], 1);
        assert_eq!(store_summary["activities"], 1);
        assert_eq!(store_summary["distinct_experiment_types"], 1);
        assert!(report.text.contains("Pass rate: 1/1"), "{}", report.text);
        assert!(
            report.text.contains("Distinct experiment types: 1"),
            "{}",
            report.text
        );
    }
}
