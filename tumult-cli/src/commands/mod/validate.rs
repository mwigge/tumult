//! Experiment validation, plugin discovery, and path-safety helpers.

use std::path::Path;

use anyhow::{bail, Context, Result};

use tumult_core::engine::{parse_experiment, resolve_config, resolve_secrets, validate_experiment};
use tumult_core::types::Provider;
use tumult_plugin::discovery::discover_all_plugins;
use tumult_plugin::registry::PluginRegistry;

// ── Validate command ──────────────────────────────────────────

/// # Errors
///
/// Returns an error if the file cannot be read, parsed, or fails validation.
#[must_use = "callers must handle validation errors"]
pub fn cmd_validate(experiment_path: &Path) -> Result<()> {
    let content = std::fs::read_to_string(experiment_path).with_context(|| {
        format!(
            "failed to read experiment file: {}",
            experiment_path.display()
        )
    })?;

    let experiment = parse_experiment(&content)
        .with_context(|| format!("failed to parse experiment: {}", experiment_path.display()))?;

    validate_experiment(&experiment)?;

    // SRE-10: Warn on unsupported provider types
    let all_activities = experiment
        .method
        .iter()
        .chain(experiment.rollbacks.iter())
        .chain(
            experiment
                .steady_state_hypothesis
                .as_ref()
                .map(|h| h.probes.iter())
                .into_iter()
                .flatten(),
        );
    for activity in all_activities {
        match &activity.provider {
            Provider::Http { .. } => {
                eprintln!(
                    "warning: activity '{}' uses HTTP provider (not yet supported at runtime)",
                    activity.name
                );
            }
            Provider::Native {
                plugin, function, ..
            } => {
                eprintln!(
                    "warning: activity '{}' uses native provider {}::{} (not yet wired to CLI executor)",
                    activity.name, plugin, function
                );
            }
            Provider::Process { .. } => {} // supported
        }
    }

    // Validate configuration references
    let config_result = resolve_config(&experiment.configuration);
    let secrets_result = resolve_secrets(&experiment.secrets);

    println!("Experiment: {}", experiment.title);
    if let Some(ref desc) = experiment.description {
        println!("Description: {desc}");
    }
    println!("Tags: {}", experiment.tags.join(", "));
    println!("Method steps: {}", experiment.method.len());
    println!("Rollback steps: {}", experiment.rollbacks.len());

    if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        println!(
            "Hypothesis: {} ({} probes)",
            hypothesis.title,
            hypothesis.probes.len()
        );
    }

    if experiment.estimate.is_some() {
        println!("Estimate: present (Phase 0)");
    }
    if experiment.baseline.is_some() {
        println!("Baseline: configured (Phase 1)");
    }
    if experiment.regulatory.is_some() {
        println!("Regulatory: mapped");
    }

    // Report config/secret resolution
    match config_result {
        Ok(_) => println!("Configuration: all values resolved"),
        Err(e) => println!("Configuration: WARNING — {e}"),
    }
    match secrets_result {
        Ok(_) => println!("Secrets: all values resolved"),
        Err(e) => println!("Secrets: WARNING — {e}"),
    }

    println!("\nValidation passed.");
    Ok(())
}

// ── Discover command ──────────────────────────────────────────

/// # Errors
///
/// Returns an error if the requested plugin filter does not match any
/// discovered plugin.
#[must_use = "callers must handle plugin discovery errors"]
pub fn cmd_discover(plugin_filter: Option<&str>) -> Result<()> {
    let mut registry = PluginRegistry::new();

    // Discover script plugins from filesystem
    let manifests = discover_all_plugins().unwrap_or_default();
    for manifest in manifests {
        registry.register_script(manifest);
    }

    let plugin_names = registry.list_plugins();

    // Check filter early — even when no plugins, a filter for a specific one should error
    if let Some(filter) = plugin_filter {
        if !plugin_names.iter().any(|n| n == filter) {
            bail!(
                "plugin '{}' not found. Discovered {} plugin(s)",
                filter,
                plugin_names.len()
            );
        }
        // Show details for specific plugin
        println!("Plugin: {filter}");
        let all_actions = registry.list_all_actions();
        let actions: Vec<_> = all_actions.iter().filter(|(p, _)| p == filter).collect();
        if !actions.is_empty() {
            println!("\nActions:");
            for (_, desc) in &actions {
                println!("  - {}", desc.name);
            }
        }
    } else {
        // List all plugins
        println!("Discovered {} plugin(s):\n", plugin_names.len());
        for name in &plugin_names {
            println!("  {name}");
        }
        println!();

        let all_actions = registry.list_all_actions();
        if !all_actions.is_empty() {
            println!("Actions:");
            for (plugin, desc) in &all_actions {
                println!("  {}::{}", plugin, desc.name);
            }
        }
    }

    Ok(())
}

// ── Path validation ─────────────────────────────────────────

/// Best-effort symlink check. Note: there is an inherent TOCTOU race
/// between this check and subsequent file operations — the path could
/// become a symlink after validation. This is acceptable for our threat
/// model (local CLI tool, not a network service). For stronger guarantees,
/// callers should use `O_NOFOLLOW` or `openat2` with `RESOLVE_NO_SYMLINKS`.
pub(crate) fn validate_path_no_symlink(path: &Path) -> Result<()> {
    if path.is_symlink() {
        bail!("symlink not allowed for security: {}", path.display());
    }
    Ok(())
}
