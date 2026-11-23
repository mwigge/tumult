//! Experiment validation, plugin discovery, and path-safety helpers.

use std::path::Path;

use anyhow::{bail, Context, Result};

use tumult_core::engine::{
    parse_experiment, resolve_config, resolve_secrets, validate_experiment, validate_regulatory,
};
use tumult_core::types::{Experiment, Provider};
use tumult_plugin::native::NativeExecutorRegistry;
use tumult_plugin::registry::PluginRegistry;
use tumult_plugin::ScriptPluginManifest;

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

    let experiment = match parse_experiment(&content) {
        Ok(experiment) => experiment,
        Err(e) => {
            if let Some(hint) = gameday_file_hint(experiment_path, &content) {
                bail!(
                    "failed to parse experiment: {}: {e}\n\nhint: {hint}",
                    experiment_path.display()
                );
            }
            return Err(e).with_context(|| {
                format!("failed to parse experiment: {}", experiment_path.display())
            });
        }
    };

    validate_experiment(&experiment)?;
    validate_regulatory(&experiment)?;

    refuse_unknown_plugin_refs(&experiment)?;

    // Validate configuration references
    let config_result = resolve_config(&experiment.configuration);
    let secrets_result = resolve_secrets(&experiment.secrets);

    crate::outln!("Experiment: {}", experiment.title);
    if let Some(ref desc) = experiment.description {
        crate::outln!("Description: {desc}");
    }
    crate::outln!("Tags: {}", experiment.tags.join(", "));
    crate::outln!("Method steps: {}", experiment.method.len());
    crate::outln!("Rollback steps: {}", experiment.rollbacks.len());

    if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        crate::outln!(
            "Hypothesis: {} ({} probes)",
            hypothesis.title,
            hypothesis.probes.len()
        );
    }

    if experiment.estimate.is_some() {
        crate::outln!("Estimate: present (Phase 0)");
    }
    if experiment.baseline.is_some() {
        crate::outln!("Baseline: declared, but a run does not use it (tolerances stay as written)");
    }
    if let Some(mapping) = &experiment.regulatory {
        crate::outln!("Regulatory: mapped");
        for requirement in &mapping.requirements {
            if let Some(id) =
                tumult_core::compliance::run_requirement_id(&mapping.frameworks, &requirement.id)
            {
                crate::outln!("  {} is the DORA requirement {id}", requirement.id);
            }
        }
    }

    // Report config/secret resolution
    match config_result {
        Ok(_) => crate::outln!("Configuration: all values resolved"),
        Err(e) => crate::outln!("Configuration: WARNING — {e}"),
    }
    match secrets_result {
        Ok(_) => crate::outln!("Secrets: all values resolved"),
        Err(e) => crate::outln!("Secrets: WARNING — {e}"),
    }

    crate::outln!("\nValidation passed.");
    Ok(())
}

/// `tumult validate` speaks the experiment schema; a `GameDay` file
/// (`.gameday.toon`, with a top-level `experiments` list) is a different
/// document and fails with a confusing "unknown field `experiments`" error.
/// Detect that case so the error can point at the right command instead.
fn gameday_file_hint(path: &Path, content: &str) -> Option<String> {
    let named_like_gameday = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".gameday.toon"));
    // TOON top-level keys are unindented; arrays render as `key[N]:`.
    let has_experiments_key = content
        .lines()
        .any(|line| line.starts_with("experiments:") || line.starts_with("experiments["));
    (named_like_gameday || has_experiments_key).then(|| {
        format!(
            "{} looks like a GameDay file, which `tumult validate` does not check \
             — run it with `tumult gameday run {}`",
            path.display(),
            path.display()
        )
    })
}

/// SRE-10: refuse an experiment that names a plugin, function or action
/// this binary cannot run. Such a step fails only when the run reaches it,
/// which can be after a fault is in place. Discovery problems are printed so
/// a skipped path or malformed manifest does not silently shrink the
/// available plugin set.
fn refuse_unknown_plugin_refs(experiment: &Experiment) -> Result<()> {
    let discovery = tumult_plugin::discovery::discover_all_report();
    for warning in &discovery.warnings {
        crate::errln!("warning: {warning}");
    }
    let scripts: Vec<&ScriptPluginManifest> =
        discovery.plugins.iter().map(|p| &p.manifest).collect();
    let faults = unknown_plugin_refs(experiment, tumult_exec::native_registry(), &scripts);
    if faults.is_empty() {
        return Ok(());
    }
    let subject = if faults.len() == 1 {
        "1 step names"
    } else {
        &format!("{} steps name", faults.len())
    };
    bail!(
        "{subject} something this binary cannot run:\n  {}",
        faults.join("\n  ")
    );
}

/// Every provider of the experiment, with the words it is reported under:
/// method, rollbacks and hypothesis probes as activities, then the probes of
/// the guards and the providers of the controls.
fn named_providers(experiment: &Experiment) -> Vec<(String, &Provider)> {
    let hypothesis_probes = experiment
        .steady_state_hypothesis
        .iter()
        .flat_map(|h| &h.probes);
    let activities = experiment
        .method
        .iter()
        .chain(&experiment.rollbacks)
        .chain(hypothesis_probes)
        .map(|a| (format!("activity '{}'", a.name), &a.provider));
    let guards = experiment
        .guards
        .iter()
        .map(|g| (format!("guard '{}'", g.name), &g.probe.provider));
    let controls = experiment
        .controls
        .iter()
        .map(|c| (format!("control '{}'", c.name), &c.provider));
    activities.chain(guards).chain(controls).collect()
}

/// One line per provider that names an unknown plugin, function or action.
fn unknown_plugin_refs(
    experiment: &Experiment,
    native: &NativeExecutorRegistry,
    scripts: &[&ScriptPluginManifest],
) -> Vec<String> {
    named_providers(experiment)
        .into_iter()
        .filter_map(|(owner, provider)| {
            let fault = match provider {
                Provider::Native {
                    plugin, function, ..
                } => native_fault(native, plugin, function),
                Provider::Script {
                    plugin, function, ..
                } => script_fault(scripts, plugin, function),
                Provider::Process { .. } => None,
            };
            fault.map(|fault| format!("{owner} uses {fault}"))
        })
        .collect()
}

fn native_fault(native: &NativeExecutorRegistry, plugin: &str, function: &str) -> Option<String> {
    let Some(executor) = native.get(plugin) else {
        return Some(format!(
            "unknown native plugin '{plugin}' (available: {})",
            native.plugin_names().join(", ")
        ));
    };
    if executor.functions().contains(&function) {
        return None;
    }
    Some(format!(
        "unknown function '{plugin}::{function}' (available: {})",
        executor.functions().join(", ")
    ))
}

fn script_fault(scripts: &[&ScriptPluginManifest], plugin: &str, function: &str) -> Option<String> {
    let Some(manifest) = scripts.iter().find(|m| m.name == plugin) else {
        let available: Vec<&str> = scripts.iter().map(|m| m.name.as_str()).collect();
        return Some(format!(
            "unknown script plugin '{plugin}' (available: {})",
            available.join(", ")
        ));
    };
    let actions = manifest.actions.iter().map(|a| a.name.as_str());
    let probes = manifest.probes.iter().map(|p| p.name.as_str());
    let known: Vec<&str> = actions.chain(probes).collect();
    if known.contains(&function) {
        return None;
    }
    Some(format!(
        "unknown action '{plugin}::{function}' (available: {})",
        known.join(", ")
    ))
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
        crate::errln!("warning: {warning}");
    }
    for discovered in discovery.plugins {
        registry.register_script(discovered.manifest);
    }

    // Native plugins come from the same composition-root registry the
    // experiment runner dispatches through, so discovery and execution
    // can never disagree.
    let output = render_discover(plugin_filter, &registry, tumult_exec::native_registry())?;
    crate::out!("{output}");
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

#[cfg(test)]
mod tests {
    use super::*;

    const GAMEDAY_TOON: &str =
        "title: Q2 Resilience\n\nexperiments[1]:\n  - path: pg-cpu-stress.toon\n";

    #[test]
    fn gameday_suffix_parse_failure_carries_gameday_run_hint() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("q2.gameday.toon");
        std::fs::write(&path, GAMEDAY_TOON).unwrap();

        let err = cmd_validate(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("looks like a GameDay file"), "{msg}");
        assert!(msg.contains("tumult gameday run"), "{msg}");
    }

    #[test]
    fn experiments_key_without_suffix_also_hints() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("campaign.toon");
        std::fs::write(&path, GAMEDAY_TOON).unwrap();

        let err = cmd_validate(&path).unwrap_err();
        assert!(err.to_string().contains("tumult gameday run"), "{err}");
    }

    #[test]
    fn ordinary_experiment_parse_failure_has_no_gameday_hint() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("experiment.toon");
        std::fs::write(&path, "unknown_field: 1\n").unwrap();

        let err = cmd_validate(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("failed to parse experiment"), "{msg}");
        assert!(!msg.contains("gameday"), "{msg}");
    }

    #[test]
    fn gameday_hint_detection_cases() {
        assert!(gameday_file_hint(Path::new("x.gameday.toon"), "title: x\n").is_some());
        assert!(gameday_file_hint(Path::new("x.toon"), GAMEDAY_TOON).is_some());
        assert!(gameday_file_hint(Path::new("x.toon"), "title: x\n").is_none());
        // An indented `experiments` line is not a top-level key.
        assert!(gameday_file_hint(Path::new("x.toon"), "  experiments: no\n").is_none());
    }
}
