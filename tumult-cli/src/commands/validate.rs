//! Experiment validation, plugin discovery, and path-safety helpers.

use std::path::Path;

use anyhow::{bail, Context, Result};

use tumult_core::engine::{parse_experiment, resolve_config, resolve_secrets, validate_experiment};
use tumult_core::types::Provider;
use tumult_plugin::native::NativeExecutorRegistry;
use tumult_plugin::registry::PluginRegistry;

// ── Validate command ──────────────────────────────────────────

/// # Errors
///
/// Returns an error if the file cannot be read, parsed, or fails validation.
#[must_use = "callers must handle validation errors"]
pub fn cmd_validate(experiment_path: &Path) -> Result<()> {
    // S-C3: File size limit before deserialization (10MB max) — same guard
    // as `cmd_run`, so a file too large to run is too large to validate.
    let file_size = std::fs::metadata(experiment_path).map_or(0, |m| m.len());
    if file_size > 10 * 1024 * 1024 {
        bail!(
            "experiment file too large ({} bytes, max 10MB): {}",
            file_size,
            experiment_path.display()
        );
    }

    let content = std::fs::read_to_string(experiment_path).with_context(|| {
        format!(
            "failed to read experiment file: {}",
            experiment_path.display()
        )
    })?;

    let experiment = parse_experiment(&content)
        .with_context(|| format!("failed to parse experiment: {}", experiment_path.display()))?;

    validate_experiment(&experiment)?;

    // SRE-10: Warn when a native activity references an unknown plugin or function.
    let native_registry = super::exec::native_registry();
    // Same check for script providers, resolved through the discovery search
    // paths; discovery problems are surfaced so a skipped path or malformed
    // manifest does not silently shrink the available plugin set.
    let discovery = tumult_plugin::discovery::discover_all_report();
    for warning in &discovery.warnings {
        eprintln!("warning: {warning}");
    }
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
            Provider::Native {
                plugin, function, ..
            } => match native_registry.get(plugin) {
                None => eprintln!(
                    "warning: activity '{}' uses unknown native plugin '{}' (available: {})",
                    activity.name,
                    plugin,
                    native_registry.plugin_names().join(", ")
                ),
                Some(executor) if !executor.functions().contains(&function.as_str()) => {
                    eprintln!(
                        "warning: activity '{}' uses unknown function '{}::{}' (available: {})",
                        activity.name,
                        plugin,
                        function,
                        executor.functions().join(", ")
                    );
                }
                Some(_) => {} // registered native plugin + function
            },
            Provider::Script {
                plugin, function, ..
            } => match discovery
                .plugins
                .iter()
                .find(|p| &p.manifest.name == plugin)
            {
                None => eprintln!(
                    "warning: activity '{}' uses unknown script plugin '{}' (available: {})",
                    activity.name,
                    plugin,
                    discovery
                        .plugins
                        .iter()
                        .map(|p| p.manifest.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                Some(discovered)
                    if !discovered
                        .manifest
                        .actions
                        .iter()
                        .any(|a| &a.name == function)
                        && !discovered
                            .manifest
                            .probes
                            .iter()
                            .any(|p| &p.name == function) =>
                {
                    eprintln!(
                        "warning: activity '{}' uses unknown action '{}::{}' (available: {})",
                        activity.name,
                        plugin,
                        function,
                        discovered
                            .manifest
                            .actions
                            .iter()
                            .map(|a| a.name.as_str())
                            .chain(discovered.manifest.probes.iter().map(|p| p.name.as_str()))
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                }
                Some(_) => {} // discovered script plugin + action
            },
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

    // Discover script plugins from filesystem. Discovery is fault-tolerant;
    // problems (unreadable paths, malformed manifests, shadowed plugins) are
    // surfaced on stderr instead of silently shrinking the listing.
    let discovery = tumult_plugin::discovery::discover_all_report();
    for warning in &discovery.warnings {
        eprintln!("warning: {warning}");
    }
    for discovered in discovery.plugins {
        registry.register_script(discovered.manifest);
    }

    // Native plugins come from the same composition-root registry the
    // experiment runner dispatches through, so discovery and execution
    // can never disagree.
    let output = render_discover(plugin_filter, &registry, super::exec::native_registry())?;
    print!("{output}");
    Ok(())
}

/// Render `tumult discover` output: script plugins from the filesystem and
/// native plugins from the executor registry, labeled by kind.
///
/// # Errors
///
/// Returns an error if `plugin_filter` does not match any plugin of either
/// kind.
pub(crate) fn render_discover(
    plugin_filter: Option<&str>,
    registry: &PluginRegistry,
    native: &NativeExecutorRegistry,
) -> Result<String> {
    use std::fmt::Write as _;

    // (name, kind) pairs, merged and sorted by name.
    let script_names = registry.list_plugins();
    let mut plugins: Vec<(String, &str)> = script_names
        .iter()
        .map(|name| (name.clone(), "script"))
        .collect();
    plugins.extend(
        native
            .plugin_names()
            .into_iter()
            .map(|name| (name.to_string(), "native")),
    );
    plugins.sort();

    // Check filter early — even when no plugins, a filter for a specific one should error
    if let Some(filter) = plugin_filter {
        let Some((name, kind)) = plugins.iter().find(|(name, _)| name == filter) else {
            bail!(
                "plugin '{}' not found. Discovered {} plugin(s)",
                filter,
                plugins.len()
            );
        };

        // Show details for specific plugin
        let mut output = format!("Plugin: {name} ({kind})\n");
        let actions: Vec<String> = match native.get(filter) {
            Some(executor) => executor
                .functions()
                .iter()
                .map(ToString::to_string)
                .collect(),
            None => registry
                .list_all_actions()
                .into_iter()
                .filter(|(plugin, _)| plugin == filter)
                .map(|(_, desc)| desc.name)
                .collect(),
        };
        if !actions.is_empty() {
            output += "\nActions:\n";
            for action in &actions {
                let _ = writeln!(output, "  - {action}");
            }
        }
        return Ok(output);
    }

    // List all plugins
    let native_count = native.plugin_names().len();
    let mut output = format!(
        "Discovered {} plugin(s) ({} script, {native_count} native):\n\n",
        plugins.len(),
        script_names.len(),
    );
    for (name, kind) in &plugins {
        let _ = writeln!(output, "  {name} ({kind})");
    }
    output += "\n";

    // Actions of both kinds, sorted for stable output.
    let mut all_actions: Vec<String> = registry
        .list_all_actions()
        .iter()
        .map(|(plugin, desc)| format!("{plugin}::{}", desc.name))
        .collect();
    all_actions.extend(native.qualified_functions());
    all_actions.sort();
    if !all_actions.is_empty() {
        let _ = writeln!(output, "Actions: {}", all_actions.len());
        for action in &all_actions {
            let _ = writeln!(output, "  {action}");
        }
    }

    Ok(output)
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
