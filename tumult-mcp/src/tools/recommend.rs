//! Advisory tools: `recommend` next tests and `coverage` reporting.

use std::fmt::Write as _;

use crate::error::ToolError;
use crate::tools::validation::validate_action_name;

/// Returns recommendations for what to test next.
///
/// # Errors
///
/// Returns a [`ToolError`] if the store cannot be opened or queried.
#[allow(clippy::too_many_lines)] // Recommendation logic covers multiple metrics and formatting stages; splitting would not reduce complexity
pub fn recommend(
    store_path: &str,
    goal: Option<&str>,
    model: Option<&str>,
    include_draft: bool,
    format: &str,
) -> Result<String, ToolError> {
    if format != "text" && format != "json" {
        return Err(ToolError::InvalidInput(format!(
            "unsupported recommend format '{format}'; expected text or json"
        )));
    }

    let path = std::path::PathBuf::from(store_path);
    if !path.exists() {
        return Ok("No analytics store found. Run some experiments first.".to_string());
    }

    let store = tumult_analytics::AnalyticsStore::open(&path)
        .map_err(|e| ToolError::Store(e.to_string()))?;

    let available_plugins = tumult_plugin::discovery::discover_all_plugins().unwrap_or_default();
    let available_actions: Vec<String> = available_plugins
        .iter()
        .flat_map(|p| {
            p.actions
                .iter()
                .map(move |a| format!("{}::{}", p.name, a.name))
        })
        .collect();

    let tested_actions = store
        .query("SELECT DISTINCT name FROM activity_results WHERE activity_type = 'action'")
        .unwrap_or_default();
    let tested_set: std::collections::HashSet<String> = tested_actions
        .into_iter()
        .filter_map(|row| row.into_iter().next())
        .collect();

    let untested: Vec<&String> = available_actions
        .iter()
        .filter(|a| {
            let short_name = a.split("::").nth(1).unwrap_or(a);
            !tested_set.contains(short_name)
        })
        .collect();

    let failures = store
        .query(
            "SELECT title, count(*) as fails FROM experiments \
             WHERE status != 'completed' GROUP BY title \
             ORDER BY fails DESC LIMIT 5",
        )
        .unwrap_or_default();

    let stale = store
        .query(
            "SELECT title, max(started_at_ns) as last_run \
             FROM experiments GROUP BY title \
             ORDER BY last_run ASC LIMIT 5",
        )
        .unwrap_or_default();

    if format == "json" {
        let failures_json: Vec<Vec<String>> =
            failures.iter().map(|row| row.as_slice().to_vec()).collect();
        let stale_json: Vec<Vec<String>> =
            stale.iter().map(|row| row.as_slice().to_vec()).collect();

        return serde_json::to_string_pretty(&serde_json::json!({
            "goal": goal,
            "model": model,
            "include_draft": include_draft,
            "coverage": {
                "tested_actions": available_actions.len().saturating_sub(untested.len()),
                "available_actions": available_actions.len(),
                "untested_actions": untested,
            },
            "failures": failures_json,
            "stale_experiments": stale_json,
            "draft": include_draft.then_some("Run coverage gaps first, then replay failing journals as deterministic regression tests."),
        }))
        .map_err(|e| ToolError::Execution(e.to_string()));
    }

    let mut output = String::new();
    writeln!(output, "=== Recommendations ===").ok();
    writeln!(output).ok();
    if let Some(goal) = goal {
        writeln!(output, "Goal: {goal}").ok();
    }
    if let Some(model) = model {
        writeln!(output, "Model: {model}").ok();
    }
    writeln!(
        output,
        "Coverage: {}/{} actions tested ({:.0}%)",
        available_actions.len() - untested.len(),
        available_actions.len(),
        if available_actions.is_empty() {
            0.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            {
                ((available_actions.len() - untested.len()) as f64 / available_actions.len() as f64)
                    * 100.0
            }
        }
    )
    .ok();

    if !untested.is_empty() {
        writeln!(output).ok();
        writeln!(output, "Untested actions ({}):", untested.len()).ok();
        for action in untested.iter().take(15) {
            writeln!(output, "  - {action}").ok();
        }
        if untested.len() > 15 {
            writeln!(output, "  ... and {} more", untested.len() - 15).ok();
        }
    }

    if !failures.is_empty() {
        writeln!(output).ok();
        writeln!(output, "Most failing experiments:").ok();
        for row in &failures {
            if row.len() >= 2 {
                writeln!(output, "  {} ({} failures)", row[0], row[1]).ok();
            }
        }
    }

    if !stale.is_empty() {
        writeln!(output).ok();
        writeln!(output, "Oldest experiments (consider re-running):").ok();
        for row in &stale {
            if !row.is_empty() {
                writeln!(output, "  - {}", row[0]).ok();
            }
        }
    }

    writeln!(output).ok();
    writeln!(output, "Suggested next steps:").ok();
    if !untested.is_empty() {
        writeln!(
            output,
            "  1. Test untested actions — {} actions have never been executed",
            untested.len()
        )
        .ok();
    }
    if !failures.is_empty() {
        writeln!(
            output,
            "  2. Investigate failures — {} experiment types have non-passing runs",
            failures.len()
        )
        .ok();
    }
    writeln!(
        output,
        "  3. Run a GameDay to validate end-to-end resilience with compliance scoring"
    )
    .ok();
    if include_draft {
        writeln!(
            output,
            "  4. Draft deterministic replay cases from any failed journals"
        )
        .ok();
    }

    Ok(output)
}

/// Returns a coverage report — which plugins, targets, and fault types
/// have been tested vs what is available.
///
/// # Errors
///
/// Returns a [`ToolError`] if the store cannot be opened or queried.
pub fn coverage(store_path: &str) -> Result<String, ToolError> {
    let path = std::path::PathBuf::from(store_path);

    // Available capabilities
    let available_plugins = tumult_plugin::discovery::discover_all_plugins().unwrap_or_default();
    let mut output = String::new();

    writeln!(output, "=== Coverage Report ===").ok();
    writeln!(output).ok();

    // Plugin-level coverage
    writeln!(output, "Plugin coverage:").ok();

    let store = if path.exists() {
        tumult_analytics::AnalyticsStore::open(&path).ok()
    } else {
        None
    };

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
    }

    // Summary stats from store
    if let Some(ref s) = store {
        writeln!(output).ok();
        writeln!(output, "Store summary:").ok();

        let experiment_count = s
            .query("SELECT count(*) FROM experiments")
            .ok()
            .and_then(|r| r.first().cloned())
            .and_then(|r| r.first().cloned())
            .unwrap_or_else(|| "0".to_string());
        let activity_count = s
            .query("SELECT count(*) FROM activity_results")
            .ok()
            .and_then(|r| r.first().cloned())
            .and_then(|r| r.first().cloned())
            .unwrap_or_else(|| "0".to_string());
        let pass_count = s
            .query("SELECT count(*) FROM experiments WHERE status = 'completed'")
            .ok()
            .and_then(|r| r.first().cloned())
            .and_then(|r| r.first().cloned())
            .unwrap_or_else(|| "0".to_string());

        writeln!(output, "  Experiments: {experiment_count}").ok();
        writeln!(output, "  Activities: {activity_count}").ok();
        writeln!(output, "  Pass rate: {pass_count}/{experiment_count}").ok();

        // Distinct targets
        let targets = s
            .query("SELECT DISTINCT title FROM experiments ORDER BY title")
            .unwrap_or_default();
        writeln!(output, "  Distinct experiment types: {}", targets.len()).ok();
    } else {
        writeln!(output).ok();
        writeln!(
            output,
            "No analytics store found. Run experiments to build coverage data."
        )
        .ok();
    }

    Ok(output)
}
