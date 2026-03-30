//! CLI command implementations.
//!
//! Each command handler takes parsed CLI arguments and orchestrates the
//! appropriate tumult-core operations.

use std::path::Path;

use tumult_core::controls::ControlRegistry;
use tumult_core::engine::{parse_experiment, resolve_config, resolve_secrets, validate_experiment};
use tumult_core::execution::RollbackStrategy;
use tumult_core::journal::write_journal;
use tumult_core::runner::{run_experiment, ActivityExecutor, ActivityOutcome, RunConfig};
use tumult_core::types::*;
use tumult_plugin::discovery::discover_all_plugins;
use tumult_plugin::registry::PluginRegistry;

use anyhow::{bail, Context, Result};

// ── Provider-based executor ───────────────────────────────────

/// Executes activities by dispatching to the appropriate provider.
///
/// For Phase 0, this supports Process and HTTP providers.
/// Native plugin execution will be wired in when native plugins ship.
pub struct ProviderExecutor;

impl ActivityExecutor for ProviderExecutor {
    fn execute(&self, activity: &Activity) -> ActivityOutcome {
        match &activity.provider {
            Provider::Process {
                path,
                arguments,
                env,
                timeout_s,
            } => execute_process(path, arguments, env, timeout_s.as_ref()),
            Provider::Http {
                method,
                url,
                headers: _,
                body: _,
                timeout_s: _,
            } => {
                // HTTP execution is deferred to Phase 1 (requires reqwest)
                // For now, return a placeholder
                ActivityOutcome {
                    success: false,
                    output: None,
                    error: Some(format!(
                        "HTTP provider not yet implemented: {} {}",
                        format_http_method(method),
                        url
                    )),
                    duration_ms: 0,
                }
            }
            Provider::Native {
                plugin, function, ..
            } => ActivityOutcome {
                success: false,
                output: None,
                error: Some(format!(
                    "Native plugin '{}::{}' not yet available — install with cargo feature flag",
                    plugin, function
                )),
                duration_ms: 0,
            },
        }
    }
}

fn format_http_method(method: &HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
        HttpMethod::Put => "PUT",
        HttpMethod::Delete => "DELETE",
        HttpMethod::Patch => "PATCH",
    }
}

fn execute_process(
    path: &str,
    arguments: &[String],
    env: &std::collections::HashMap<String, String>,
    timeout_s: Option<&f64>,
) -> ActivityOutcome {
    let start = std::time::Instant::now();

    let mut cmd = std::process::Command::new(path);
    cmd.args(arguments);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return ActivityOutcome {
                success: false,
                output: None,
                error: Some(format!("failed to execute '{}': {}", path, e)),
                duration_ms: start.elapsed().as_millis() as u64,
            };
        }
    };

    // Apply timeout if configured
    let timeout = timeout_s.map(|s| std::time::Duration::from_secs_f64(*s));
    let status = if let Some(dur) = timeout {
        // Poll until done or timeout
        let deadline = std::time::Instant::now() + dur;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err("timed out".to_string());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(e) => break Err(e.to_string()),
            }
        }
    } else {
        child.wait().map_err(|e| e.to_string())
    };

    let duration_ms = start.elapsed().as_millis() as u64;

    match status {
        Ok(exit_status) => {
            let stdout = child
                .stdout
                .map(|mut s| {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut s, &mut buf).ok();
                    buf.trim().to_string()
                })
                .unwrap_or_default();
            let stderr = child
                .stderr
                .map(|mut s| {
                    let mut buf = String::new();
                    std::io::Read::read_to_string(&mut s, &mut buf).ok();
                    buf.trim().to_string()
                })
                .unwrap_or_default();

            ActivityOutcome {
                success: exit_status.success(),
                output: if stdout.is_empty() {
                    None
                } else {
                    Some(stdout)
                },
                error: if stderr.is_empty() {
                    None
                } else {
                    Some(stderr)
                },
                duration_ms,
            }
        }
        Err(reason) => ActivityOutcome {
            success: false,
            output: None,
            error: Some(format!("process '{}' {}", path, reason)),
            duration_ms,
        },
    }
}

// ── Run command ───────────────────────────────────────────────

pub fn cmd_run(
    experiment_path: &Path,
    journal_path: &Path,
    dry_run: bool,
    rollback_strategy: RollbackStrategy,
) -> Result<()> {
    let content = std::fs::read_to_string(experiment_path).with_context(|| {
        format!(
            "failed to read experiment file: {}",
            experiment_path.display()
        )
    })?;

    let experiment = parse_experiment(&content)
        .with_context(|| format!("failed to parse experiment: {}", experiment_path.display()))?;

    validate_experiment(&experiment)?;

    // Resolve configuration and secrets
    let _config = resolve_config(&experiment.configuration)?;
    let _secrets = resolve_secrets(&experiment.secrets)?;

    if dry_run {
        print_dry_run(&experiment);
        return Ok(());
    }

    let executor = ProviderExecutor;
    let controls = ControlRegistry::new();
    let run_config = RunConfig { rollback_strategy };

    println!("Running experiment: {}", experiment.title);

    let journal = run_experiment(&experiment, &executor, &controls, &run_config)?;

    write_journal(&journal, journal_path)?;

    println!("Status: {:?}", journal.status);
    println!("Duration: {}ms", journal.duration_ms);
    println!("Method steps: {} executed", journal.method_results.len());
    if !journal.rollback_results.is_empty() {
        println!("Rollbacks: {} executed", journal.rollback_results.len());
    }
    println!("Journal written to: {}", journal_path.display());

    // Exit with non-zero if experiment did not complete successfully
    if journal.status != ExperimentStatus::Completed {
        bail!("experiment finished with status: {:?}", journal.status);
    }

    Ok(())
}

// ── Validate command ──────────────────────────────────────────

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

    // Validate configuration references
    let config_result = resolve_config(&experiment.configuration);
    let secrets_result = resolve_secrets(&experiment.secrets);

    println!("Experiment: {}", experiment.title);
    if let Some(ref desc) = experiment.description {
        println!("Description: {}", desc);
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
        Err(e) => println!("Configuration: WARNING — {}", e),
    }
    match secrets_result {
        Ok(_) => println!("Secrets: all values resolved"),
        Err(e) => println!("Secrets: WARNING — {}", e),
    }

    println!("\nValidation passed.");
    Ok(())
}

// ── Discover command ──────────────────────────────────────────

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
        println!("Plugin: {}", filter);
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
            println!("  {}", name);
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

// ── Init command ──────────────────────────────────────────────

pub fn cmd_init(plugin: Option<&str>) -> Result<()> {
    init_at(Path::new("experiment.toon"), plugin)
}

fn init_at(path: &Path, plugin: Option<&str>) -> Result<()> {
    if path.exists() {
        bail!(
            "{} already exists — remove it first or use a different name",
            path.display()
        );
    }

    let template = generate_template(plugin);
    std::fs::write(path, &template)?;

    println!("Created {}", path.display());
    if let Some(p) = plugin {
        println!("Template includes {} plugin actions", p);
    }
    println!("Edit the file to configure your experiment, then run:");
    println!("  tumult run {}", path.display());

    Ok(())
}

fn generate_template(plugin: Option<&str>) -> String {
    let plugin_name = plugin.unwrap_or("tumult-example");
    format!(
        r#"title: System information check
description: Verify system is accessible and report CPU and memory info

tags[2]: resilience, baseline

steady_state_hypothesis:
  title: System is reachable
  probes[1]:
    - name: system-check
      activity_type: probe
      provider:
        type: process
        path: uname
        arguments[1]: "-a"
        timeout_s: 5.0
      tolerance:
        type: regex
        pattern: "."

method[2]:
  - name: check-cpu
    activity_type: probe
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "cat /proc/cpuinfo 2>/dev/null | head -20 || sysctl -n machdep.cpu.brand_string"
      timeout_s: 10.0
  - name: check-memory
    activity_type: probe
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "cat /proc/meminfo 2>/dev/null | head -5 || sysctl -n hw.memsize"
      timeout_s: 10.0

rollbacks[1]:
  - name: log-complete
    activity_type: action
    provider:
      type: process
      path: echo
      arguments[1]: "system check completed via {plugin_name}"
      timeout_s: 5.0
"#
    )
}

// ── Dry run ───────────────────────────────────────────────────

fn print_dry_run(experiment: &Experiment) {
    println!("=== DRY RUN ===\n");
    println!("Experiment: {}", experiment.title);
    if let Some(ref desc) = experiment.description {
        println!("Description: {}", desc);
    }
    println!();

    if let Some(ref estimate) = experiment.estimate {
        println!("Phase 0 — Estimate:");
        println!("  Expected outcome: {:?}", estimate.expected_outcome);
        if let Some(recovery) = estimate.expected_recovery_s {
            println!("  Expected recovery: {}s", recovery);
        }
        println!();
    }

    if let Some(ref baseline) = experiment.baseline {
        println!("Phase 1 — Baseline:");
        println!("  Duration: {}s", baseline.duration_s);
        println!("  Interval: {}s", baseline.interval_s);
        println!("  Method: {:?}", baseline.method);
        println!();
    }

    if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        println!("Hypothesis: {}", hypothesis.title);
        for probe in &hypothesis.probes {
            println!("  Probe: {}", probe.name);
        }
        println!();
    }

    println!("Phase 2 — Method ({} steps):", experiment.method.len());
    for (i, activity) in experiment.method.iter().enumerate() {
        let bg = if activity.background {
            " [background]"
        } else {
            ""
        };
        println!(
            "  {}. {} ({:?}){}",
            i + 1,
            activity.name,
            activity.activity_type,
            bg
        );
    }
    println!();

    if !experiment.rollbacks.is_empty() {
        println!("Rollbacks ({} steps):", experiment.rollbacks.len());
        for activity in &experiment.rollbacks {
            println!("  - {} ({:?})", activity.name, activity.activity_type);
        }
        println!();
    }

    if let Some(ref regulatory) = experiment.regulatory {
        println!("Regulatory: {}", regulatory.frameworks.join(", "));
    }

    println!("=== END DRY RUN ===");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ── Helper: write a valid experiment file ─────────────────

    fn write_valid_experiment(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("test-experiment.toon");
        let exp = Experiment {
            title: "CLI test experiment".into(),
            description: Some("Tests CLI command execution".into()),
            tags: vec!["test".into()],
            configuration: std::collections::HashMap::new(),
            secrets: std::collections::HashMap::new(),
            controls: vec![],
            steady_state_hypothesis: None,
            method: vec![Activity {
                name: "echo-action".into(),
                activity_type: ActivityType::Action,
                provider: Provider::Process {
                    path: "echo".into(),
                    arguments: vec!["hello".into()],
                    env: std::collections::HashMap::new(),
                    timeout_s: Some(5.0),
                },
                tolerance: None,
                pause_before_s: None,
                pause_after_s: None,
                background: false,
            }],
            rollbacks: vec![],
            estimate: None,
            baseline: None,
            load: None,
            regulatory: None,
        };
        let toon = toon_format::encode_default(&exp).unwrap();
        std::fs::write(&path, toon).unwrap();
        path
    }

    fn write_invalid_experiment(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("invalid.toon");
        std::fs::write(&path, "this is not valid toon {{{").unwrap();
        path
    }

    fn write_empty_method_experiment(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("empty-method.toon");
        let exp = Experiment {
            title: "Empty method experiment".into(),
            description: None,
            tags: vec![],
            configuration: std::collections::HashMap::new(),
            secrets: std::collections::HashMap::new(),
            controls: vec![],
            steady_state_hypothesis: None,
            method: vec![],
            rollbacks: vec![],
            estimate: None,
            baseline: None,
            load: None,
            regulatory: None,
        };
        let toon = toon_format::encode_default(&exp).unwrap();
        std::fs::write(&path, toon).unwrap();
        path
    }

    // ── cmd_run tests ─────────────────────────────────────────

    #[test]
    fn run_valid_experiment_produces_journal() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(dir.path());
        let journal_path = dir.path().join("journal.toon");

        let result = cmd_run(
            &exp_path,
            &journal_path,
            false,
            RollbackStrategy::OnDeviation,
        );

        assert!(result.is_ok());
        assert!(journal_path.exists());
    }

    #[test]
    fn run_dry_run_does_not_create_journal() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(dir.path());
        let journal_path = dir.path().join("journal.toon");

        let result = cmd_run(
            &exp_path,
            &journal_path,
            true,
            RollbackStrategy::OnDeviation,
        );

        assert!(result.is_ok());
        assert!(!journal_path.exists());
    }

    #[test]
    fn run_nonexistent_file_returns_error() {
        let result = cmd_run(
            Path::new("/nonexistent/experiment.toon"),
            Path::new("journal.toon"),
            false,
            RollbackStrategy::OnDeviation,
        );
        assert!(result.is_err());
    }

    #[test]
    fn run_invalid_toon_returns_error() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_invalid_experiment(dir.path());

        let result = cmd_run(
            &exp_path,
            &dir.path().join("journal.toon"),
            false,
            RollbackStrategy::OnDeviation,
        );
        assert!(result.is_err());
    }

    #[test]
    fn run_empty_method_returns_error() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_empty_method_experiment(dir.path());

        let result = cmd_run(
            &exp_path,
            &dir.path().join("journal.toon"),
            false,
            RollbackStrategy::OnDeviation,
        );
        assert!(result.is_err());
    }

    // ── cmd_validate tests ────────────────────────────────────

    #[test]
    fn validate_valid_experiment_succeeds() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_valid_experiment(dir.path());

        let result = cmd_validate(&exp_path);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_nonexistent_file_returns_error() {
        let result = cmd_validate(Path::new("/nonexistent/experiment.toon"));
        assert!(result.is_err());
    }

    #[test]
    fn validate_invalid_toon_returns_error() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_invalid_experiment(dir.path());

        let result = cmd_validate(&exp_path);
        assert!(result.is_err());
    }

    #[test]
    fn validate_empty_method_returns_error() {
        let dir = TempDir::new().unwrap();
        let exp_path = write_empty_method_experiment(dir.path());

        let result = cmd_validate(&exp_path);
        assert!(result.is_err());
    }

    // ── cmd_discover tests ────────────────────────────────────

    #[test]
    fn discover_without_plugins_shows_empty() {
        // No plugins in default search paths during tests
        let result = cmd_discover(None);
        assert!(result.is_ok());
    }

    #[test]
    fn discover_nonexistent_plugin_returns_error() {
        let result = cmd_discover(Some("nonexistent-plugin"));
        assert!(result.is_err());
    }

    // ── cmd_init tests ────────────────────────────────────────

    #[test]
    fn init_creates_experiment_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("experiment.toon");

        let result = init_at(&path, None);

        assert!(result.is_ok());
        assert!(path.exists());
    }

    #[test]
    fn init_with_plugin_includes_plugin_name() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("experiment.toon");

        let result = init_at(&path, Some("tumult-kafka"));

        assert!(result.is_ok());
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("tumult-kafka"));
    }

    #[test]
    fn init_fails_if_file_exists() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("experiment.toon");
        std::fs::write(&path, "existing").unwrap();

        let result = init_at(&path, None);
        assert!(result.is_err());
    }

    // ── generate_template tests ───────────────────────────────

    #[test]
    fn template_contains_required_sections() {
        let template = generate_template(None);
        assert!(template.contains("title:"));
        assert!(template.contains("steady_state_hypothesis:"));
        assert!(template.contains("method"));
        assert!(template.contains("rollbacks"));
    }

    #[test]
    fn template_uses_plugin_name() {
        let template = generate_template(Some("tumult-db"));
        assert!(template.contains("tumult-db"));
    }

    // ── ProviderExecutor tests ────────────────────────────────

    #[test]
    fn process_executor_runs_echo() {
        let activity = Activity {
            name: "echo-test".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "echo".into(),
                arguments: vec!["hello world".into()],
                env: std::collections::HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        };

        let executor = ProviderExecutor;
        let outcome = executor.execute(&activity);

        assert!(outcome.success);
        assert_eq!(outcome.output.as_deref(), Some("hello world"));
    }

    #[test]
    fn process_executor_captures_failure() {
        let activity = Activity {
            name: "false-test".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "false".into(),
                arguments: vec![],
                env: std::collections::HashMap::new(),
                timeout_s: Some(5.0),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        };

        let executor = ProviderExecutor;
        let outcome = executor.execute(&activity);

        assert!(!outcome.success);
    }

    #[test]
    fn process_executor_nonexistent_returns_error() {
        let activity = Activity {
            name: "bad-cmd".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Process {
                path: "/nonexistent/binary".into(),
                arguments: vec![],
                env: std::collections::HashMap::new(),
                timeout_s: None,
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        };

        let executor = ProviderExecutor;
        let outcome = executor.execute(&activity);

        assert!(!outcome.success);
        assert!(outcome.error.is_some());
    }

    #[test]
    fn native_provider_returns_not_implemented() {
        let activity = Activity {
            name: "native-test".into(),
            activity_type: ActivityType::Action,
            provider: Provider::Native {
                plugin: "test-plugin".into(),
                function: "test-fn".into(),
                arguments: std::collections::HashMap::new(),
            },
            tolerance: None,
            pause_before_s: None,
            pause_after_s: None,
            background: false,
        };

        let executor = ProviderExecutor;
        let outcome = executor.execute(&activity);

        assert!(!outcome.success);
        assert!(outcome
            .error
            .as_ref()
            .unwrap()
            .contains("not yet available"));
    }
}
