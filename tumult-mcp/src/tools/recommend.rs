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
