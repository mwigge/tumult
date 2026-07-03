//! Deterministic heuristic reporting derived from the analytics store and
//! plugin catalog.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::Path;

#[must_use]
pub fn heuristic_report(store_path: &Path) -> String {
    let mut output = String::new();
    let available_plugins = tumult_plugin::discovery::discover_all_plugins().unwrap_or_default();
    let available_actions: Vec<String> = available_plugins
        .iter()
        .flat_map(|plugin| {
            plugin
                .actions
                .iter()
                .map(move |action| format!("{}::{}", plugin.name, action.name))
        })
        .collect();

    writeln!(output, "=== Recommendations ===").ok();
    writeln!(output).ok();

    if !store_path.exists() {
        writeln!(
            output,
            "No analytics store found at {}. Run experiments to build history.",
            store_path.display()
        )
        .ok();
        writeln!(output, "Available actions: {}", available_actions.len()).ok();
        for action in available_actions.iter().take(15) {
            writeln!(output, "  - {action}").ok();
        }
        return output;
    }

    let Ok(store) = tumult_analytics::AnalyticsStore::open(store_path) else {
        writeln!(output, "Analytics store could not be opened.").ok();
        return output;
    };

    let tested_actions = store
        .query("SELECT DISTINCT name FROM activity_results WHERE activity_type = 'action'")
        .unwrap_or_default();
    let tested_set: HashSet<String> = tested_actions
        .into_iter()
        .filter_map(|row| row.into_iter().next())
        .collect();
    let untested: Vec<&String> = available_actions
        .iter()
        .filter(|action| {
            let short_name = action.split("::").nth(1).unwrap_or(action);
            !tested_set.contains(short_name)
        })
        .collect();

    let tested_count = available_actions.len().saturating_sub(untested.len());
    let coverage = if available_actions.is_empty() {
        0.0
    } else {
        #[allow(clippy::cast_precision_loss)]
        {
            (tested_count as f64 / available_actions.len() as f64) * 100.0
        }
    };
    writeln!(
        output,
        "Coverage: {tested_count}/{} actions tested ({coverage:.0}%)",
        available_actions.len()
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

    let failures = store
        .query(
            "SELECT title, count(*) as fails FROM experiments \
             WHERE status != 'completed' GROUP BY title \
             ORDER BY fails DESC LIMIT 5",
        )
        .unwrap_or_default();
    if !failures.is_empty() {
        writeln!(output).ok();
        writeln!(output, "Most failing experiments:").ok();
        for row in &failures {
            if row.len() >= 2 {
                writeln!(output, "  {} ({} failures)", row[0], row[1]).ok();
            }
        }
    }

    let stale = store
        .query(
            "SELECT title, max(started_at_ns) as last_run \
             FROM experiments GROUP BY title \
             ORDER BY last_run ASC LIMIT 5",
        )
        .unwrap_or_default();
    if !stale.is_empty() {
        writeln!(output).ok();
        writeln!(output, "Oldest experiments:").ok();
        for row in &stale {
            if let Some(title) = row.first() {
                writeln!(output, "  - {title}").ok();
            }
        }
    }

    output
}

pub(crate) fn plugin_catalog() -> String {
    let plugins = tumult_plugin::discovery::discover_all_plugins().unwrap_or_default();
    let mut output = String::new();
    for plugin in plugins {
        writeln!(output, "plugin: {}", plugin.name).ok();
        if !plugin.actions.is_empty() {
            writeln!(output, "  actions:").ok();
            for action in plugin.actions {
                writeln!(output, "    - {}: {}", action.name, action.description).ok();
            }
        }
        if !plugin.probes.is_empty() {
            writeln!(output, "  probes:").ok();
            for probe in plugin.probes {
                writeln!(output, "    - {}: {}", probe.name, probe.description).ok();
            }
        }
    }
    output
}
