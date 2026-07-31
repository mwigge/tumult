use anyhow::{bail, Context, Result};

use tumult_exec::ProviderExecutor;

use super::load::K6LoadExecutor;
use super::{ComplianceFramework, LoadToolArg};

// ── GameDay commands ────────────────────────────────────────

/// Creates a `.gameday.toon` file from experiment paths.
///
/// # Errors
///
/// Returns an error if the name is not a plain file-name component or the
/// file cannot be written.
#[must_use = "callers must handle gameday creation errors"]
pub fn cmd_gameday_create(
    name: &str,
    experiments: &[std::path::PathBuf],
    load_tool: Option<LoadToolArg>,
    load_script: Option<&std::path::Path>,
    load_vus: Option<u32>,
    framework: Option<ComplianceFramework>,
) -> Result<()> {
    use tumult_core::types::{gameday_toon_template, GameDayTemplateSpec, LoadTool};

    // The name becomes `{name}.gameday.toon` in the current directory — it
    // must be a plain file-name component, not a path (`../x` would escape
    // the working directory).
    if name.is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.contains('\0')
    {
        bail!(
            "invalid gameday name {name:?}: must be a plain file name \
             (no path separators or '..')"
        );
    }

    let core_load_tool = match load_tool {
        Some(LoadToolArg::K6) => Some(LoadTool::K6),
        Some(LoadToolArg::None) | None => None,
    };
    let framework_report_str = framework.map(|fw| fw.as_report_str());
    let content = gameday_toon_template(&GameDayTemplateSpec {
        name,
        experiments,
        load_tool: core_load_tool,
        load_script,
        load_vus,
        framework_report_str,
    });

    let filename = format!("{name}.gameday.toon");
    std::fs::write(&filename, &content).with_context(|| format!("failed to write {filename}"))?;

    println!("Created: {filename}");
    println!("Edit the file to add compliance_maps and regulatory requirements.");
    println!("Run with: tumult gameday run {filename}");
    Ok(())
}

/// Runs a `GameDay` — executes all experiments under shared load.
///
/// # Errors
///
/// Returns an error if the `GameDay` file cannot be read, parsed, or experiments fail.
#[allow(clippy::too_many_lines)] // GameDay orchestration spans load setup, multi-experiment execution, and scoring summary
#[must_use = "callers must handle gameday run errors"]
pub fn cmd_gameday_run(gameday_path: &std::path::Path) -> Result<()> {
    use tumult_core::controls::{ControlRegistry, ProviderControl};
    use tumult_core::engine::{
        apply_template_vars, build_config_env, build_secret_env, flatten_secrets, parse_experiment,
        resolve_config, resolve_secrets, validate_experiment,
    };
    use tumult_core::runner::{run_gameday_with_wiring, ExperimentWiring, RunConfig};
    use tumult_core::types::GameDay;

    let content = std::fs::read_to_string(gameday_path)
        .with_context(|| format!("failed to read: {}", gameday_path.display()))?;

    let gameday: GameDay =
        toon_format::decode_default(&content).with_context(|| "failed to parse gameday file")?;

    println!("GameDay: {}", gameday.title);
    println!("Experiments: {}", gameday.experiments.len());

    // Parse all experiment files. Each experiment gets the same
    // configuration/secrets semantics as `tumult run`: values resolve up
    // front (a missing env var or secret file is fatal before anything
    // executes), template substitution applies, and the resolved pairs are
    // injected into provider subprocesses as TUMULT_CONFIG_* / TUMULT_SECRET_*.
    let gameday_dir = gameday_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let no_vars: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut experiments = Vec::with_capacity(gameday.experiments.len());
    let mut wirings = Vec::with_capacity(gameday.experiments.len());
    for gd_exp in &gameday.experiments {
        let exp_path = if gd_exp.path.is_absolute() {
            gd_exp.path.clone()
        } else {
            gameday_dir.join(&gd_exp.path)
        };
        let exp_content = std::fs::read_to_string(&exp_path)
            .with_context(|| format!("failed to read experiment: {}", exp_path.display()))?;
        let experiment = parse_experiment(&exp_content)
            .with_context(|| format!("failed to parse: {}", exp_path.display()))?;

        let config = resolve_config(&experiment.configuration)
            .with_context(|| format!("failed to resolve configuration: {}", exp_path.display()))?;
        let secrets = resolve_secrets(&experiment.secrets)
            .with_context(|| format!("failed to resolve secrets: {}", exp_path.display()))?;
        let secrets_flat = flatten_secrets(&secrets);
        let experiment = if config.is_empty() && secrets_flat.is_empty() {
            experiment
        } else {
            apply_template_vars(&experiment, &no_vars, &config, &secrets_flat).with_context(
                || format!("failed to apply template variables: {}", exp_path.display()),
            )?
        };
        validate_experiment(&experiment)
            .with_context(|| format!("invalid experiment: {}", exp_path.display()))?;

        // Per-experiment wiring: an executor carrying this experiment's
        // injected env, and a registry of its declared controls. The warning
        // names keys only, never values — the maps carry resolved secrets.
        let (config_env, skipped_config) = build_config_env(&config);
        let (secret_env, skipped_secrets) = build_secret_env(&secrets_flat);
        for key in &skipped_config {
            eprintln!(
                "warning: configuration key '{key}' does not form a valid env var name after \
                 uppercasing; usable in templates but not injected as TUMULT_CONFIG_*"
            );
        }
        for key in &skipped_secrets {
            eprintln!(
                "warning: secret key '{key}' does not form a valid env var name after \
                 uppercasing; usable in templates but not injected as TUMULT_SECRET_*"
            );
        }
        let mut injected_env = config_env;
        injected_env.extend(secret_env);

        let executor: std::sync::Arc<dyn tumult_core::runner::ActivityExecutor> =
            std::sync::Arc::new(ProviderExecutor::with_injected_env(injected_env));
        let mut controls = ControlRegistry::new();
        for control in &experiment.controls {
            controls.register(Box::new(ProviderControl::new(
                control.clone(),
                executor.clone(),
            )));
        }
        wirings.push(ExperimentWiring {
            executor,
            controls: std::sync::Arc::new(controls),
        });
        experiments.push(experiment);
    }

    // Create load executor if gameday has load config
    let load_executor: Option<std::sync::Arc<dyn tumult_core::runner::LoadExecutor>> =
        if gameday.load.is_some() {
            Some(std::sync::Arc::new(K6LoadExecutor))
        } else {
            None
        };

    // Spawn a task that cancels the gameday if SIGINT (Ctrl-C) is received.
    let cancel_token = tokio_util::sync::CancellationToken::new();
    let cancel_token_for_signal = cancel_token.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::warn!("SIGINT received — cancelling gameday");
            cancel_token_for_signal.cancel();
        }
    });

    let config = RunConfig {
        rollback_strategy: tumult_core::execution::RollbackStrategy::OnDeviation,
        cancellation_token: Some(cancel_token),
        parent_context: None,
        load_executor,
        max_concurrent_faults: None,
    };

    println!("Running...\n");

    let journal = run_gameday_with_wiring(
        &gameday,
        &experiments,
        &|index, _| wirings[index].clone(),
        &config,
    )?;

    // Print summary
    println!("GameDay: {}", journal.title);
    println!(
        "Status:  {}/{} PASS ({})",
        journal
            .experiment_journals
            .iter()
            .filter(|j| j.status == tumult_core::types::ExperimentStatus::Completed)
            .count(),
        journal.experiment_journals.len(),
        journal.compliance_status
    );
    println!("Duration: {:.1}s\n", journal.duration_s);

    for (i, ej) in journal.experiment_journals.iter().enumerate() {
        let icon = if ej.status == tumult_core::types::ExperimentStatus::Completed {
            "PASS"
        } else {
            "FAIL"
        };
        println!(
            "  #{} [{}] {} ({}ms)",
            i + 1,
            icon,
            ej.experiment_title,
            ej.duration_ms
        );
    }

    println!(
        "\nResilience Score: {:.2}",
        journal.resilience_score.overall
    );
    println!(
        "  Pass rate:        {:.2}",
        journal.resilience_score.pass_rate
    );
    println!(
        "  Recovery:         {:.2}",
        journal.resilience_score.recovery_compliance
    );
    println!(
        "  Load impact:      {:.2}",
        journal.resilience_score.load_impact_tolerance
    );
    println!(
        "  Compliance:       {:.2}",
        journal.resilience_score.compliance_coverage
    );

    if let Some(ref lr) = journal.load_result {
        println!(
            "\nLoad ({}): {} requests, p95={}ms, error_rate={:.4}",
            lr.tool, lr.total_requests, lr.latency_p95_ms, lr.error_rate
        );
    }

    // Write gameday journal
    let journal_path = gameday_path.with_extension("journal.toon");
    let toon = toon_format::encode_default(&journal)
        .with_context(|| "failed to encode gameday journal")?;
    std::fs::write(&journal_path, &toon)
        .with_context(|| format!("failed to write {}", journal_path.display()))?;
    println!("\nJournal: {}", journal_path.display());

    Ok(())
}

/// Analyzes a completed `GameDay` journal.
///
/// # Errors
///
/// Returns an error if the journal cannot be read or parsed.
#[must_use = "callers must handle gameday analysis errors"]
pub fn cmd_gameday_analyze(gameday_path: &std::path::Path) -> Result<()> {
    use tumult_core::types::GameDayJournal;

    let journal_path = gameday_path.with_extension("journal.toon");
    let content = std::fs::read_to_string(&journal_path)
        .with_context(|| format!("failed to read: {}", journal_path.display()))?;

    let journal: GameDayJournal =
        toon_format::decode_default(&content).with_context(|| "failed to parse gameday journal")?;

    println!("GameDay: {}", journal.title);
    println!(
        "Status:  {} ({:.1}s)\n",
        journal.compliance_status, journal.duration_s
    );

    for (i, ej) in journal.experiment_journals.iter().enumerate() {
        let icon = if ej.status == tumult_core::types::ExperimentStatus::Completed {
            "PASS"
        } else {
            "FAIL"
        };
        println!(
            "  #{} [{}] {} ({}ms)",
            i + 1,
            icon,
            ej.experiment_title,
            ej.duration_ms
        );
    }

    println!(
        "\nResilience Score: {:.2}",
        journal.resilience_score.overall
    );
    println!(
        "  Pass rate:    {:.2}  Recovery: {:.2}  Load: {:.2}  Compliance: {:.2}",
        journal.resilience_score.pass_rate,
        journal.resilience_score.recovery_compliance,
        journal.resilience_score.load_impact_tolerance,
        journal.resilience_score.compliance_coverage
    );
    println!("  Status: {}", journal.compliance_status);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gameday_create_rejects_path_traversal_names() {
        for bad in ["../escape", "sub/dir", "..", "a\\b", ""] {
            let result = cmd_gameday_create(bad, &[], None, None, None, None);
            assert!(result.is_err(), "{bad:?} must be rejected");
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("invalid gameday name"),
                "{bad:?}"
            );
        }
    }

    /// End-to-end proof for the gameday config/secrets/controls wiring:
    /// each experiment's resolved configuration is injected into provider
    /// subprocesses as `TUMULT_CONFIG_*`, and its declared controls fire at
    /// lifecycle events (previously the gameday path injected nothing and
    /// built an empty registry).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn gameday_run_injects_config_env_and_fires_declared_controls() {
        const CFG_ENV_VAR: &str = "TEST_TUMULT_GAMEDAY_CFG";
        std::env::set_var(CFG_ENV_VAR, "gameday-wired");

        let dir = tempfile::TempDir::new().unwrap();
        let events_file = dir.path().join("control-events.txt");
        let exp_path = dir.path().join("exp.toon");
        let experiment = format!(
            r#"title: gameday wiring experiment

tags[1]: test

configuration:
  marker:
    type: env
    key: {CFG_ENV_VAR}

controls[1]:
  - name: event-recorder
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "echo \"$TUMULT_CONTROL_EVENT\" >> {}"
      timeout_s: 5.0

method[1]:
  - name: read-injected-env
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "echo \"cfg=$TUMULT_CONFIG_MARKER\""
      timeout_s: 5.0
"#,
            events_file.display()
        );
        std::fs::write(&exp_path, experiment).unwrap();

        let gameday_path = dir.path().join("wiring.gameday.toon");
        std::fs::write(
            &gameday_path,
            "title: wiring gameday\ndescription: test gameday\n\ntags[1]: test\n\nexperiments[1]:\n  - path: exp.toon\n    compliance_maps[0]:\n\nscoring:\n  pass_threshold: 0.75\n  mttr_target_s: 30.0\n  recovery_required: true\n",
        )
        .unwrap();

        let result = cmd_gameday_run(&gameday_path);
        std::env::remove_var(CFG_ENV_VAR);
        result.unwrap();

        // The resolved config reached the method subprocess as TUMULT_CONFIG_*.
        let journal = std::fs::read_to_string(gameday_path.with_extension("journal.toon")).unwrap();
        assert!(
            journal.contains("cfg=gameday-wired"),
            "TUMULT_CONFIG_* env injection missing from gameday journal: {journal}"
        );

        // The declared control fired at lifecycle events.
        let events = std::fs::read_to_string(&events_file).unwrap();
        for expected in ["before_experiment", "before_method", "after_experiment"] {
            assert!(
                events.contains(expected),
                "declared control did not fire on {expected}; events file: {events}"
            );
        }
    }
}
