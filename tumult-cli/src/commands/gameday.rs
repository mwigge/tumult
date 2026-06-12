use anyhow::{Context, Result};

use super::exec::ProviderExecutor;
use super::load::K6LoadExecutor;
use super::{ComplianceFramework, LoadToolArg};

// ── GameDay commands ────────────────────────────────────────

/// Creates a `.gameday.toon` file from experiment paths.
///
/// # Errors
///
/// Returns an error if the file cannot be written.
#[must_use = "callers must handle gameday creation errors"]
pub fn cmd_gameday_create(
    name: &str,
    experiments: &[std::path::PathBuf],
    load_tool: Option<LoadToolArg>,
    load_script: Option<&std::path::Path>,
    load_vus: Option<u32>,
    framework: Option<ComplianceFramework>,
) -> Result<()> {
    use std::fmt::Write;
    let mut content = String::new();

    writeln!(content, "title: {name}").ok();
    writeln!(content, "description: GameDay campaign\n").ok();
    writeln!(content, "tags[1]: gameday\n").ok();

    if let Some(ref tool) = load_tool {
        if !matches!(tool, LoadToolArg::None) {
            let tool_str = match tool {
                LoadToolArg::K6 => "k6",
                LoadToolArg::Jmeter => "jmeter",
                LoadToolArg::None => unreachable!(),
            };
            writeln!(content, "load:").ok();
            writeln!(content, "  tool: {tool_str}").ok();
            if let Some(script) = load_script {
                writeln!(content, "  script: {}", script.display()).ok();
            }
            if let Some(vus) = load_vus {
                writeln!(content, "  vus: {vus}").ok();
            }
            writeln!(content, "  duration_s: 120.0\n").ok();
        }
    }

    if let Some(fw) = framework {
        let fw_str = fw.as_report_str();
        writeln!(content, "regulatory:").ok();
        writeln!(content, "  frameworks[1]: {fw_str}").ok();
        writeln!(content, "  requirements[0]:\n").ok();
    }

    writeln!(content, "experiments[{}]:", experiments.len()).ok();
    for exp in experiments {
        writeln!(content, "  - path: {}", exp.display()).ok();
        writeln!(content, "    compliance_maps[0]:").ok();
    }

    writeln!(content, "\nscoring:").ok();
    writeln!(content, "  pass_threshold: 0.75").ok();
    writeln!(content, "  mttr_target_s: 30.0").ok();
    writeln!(content, "  recovery_required: true").ok();

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
    use tumult_core::controls::ControlRegistry;
    use tumult_core::engine::parse_experiment;
    use tumult_core::runner::{run_gameday, RunConfig};
    use tumult_core::types::GameDay;

    let content = std::fs::read_to_string(gameday_path)
        .with_context(|| format!("failed to read: {}", gameday_path.display()))?;

    let gameday: GameDay =
        toon_format::decode_default(&content).with_context(|| "failed to parse gameday file")?;

    println!("GameDay: {}", gameday.title);
    println!("Experiments: {}", gameday.experiments.len());

    // Parse all experiment files
    let gameday_dir = gameday_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut experiments = Vec::with_capacity(gameday.experiments.len());
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
        experiments.push(experiment);
    }

    let executor = ProviderExecutor;
    let controls = ControlRegistry::new();

    // Create load executor if gameday has load config
    let load_executor: Option<std::sync::Arc<dyn tumult_core::runner::LoadExecutor>> =
        if gameday.load.is_some() {
            Some(std::sync::Arc::new(K6LoadExecutor))
        } else {
            None
        };

    let config = RunConfig {
        rollback_strategy: tumult_core::execution::RollbackStrategy::OnDeviation,
        cancellation_token: None,
        parent_context: None,
        load_executor,
    };

    let executor_arc: std::sync::Arc<dyn tumult_core::runner::ActivityExecutor> =
        std::sync::Arc::new(executor);
    let controls_arc = std::sync::Arc::new(controls);

    println!("Running...\n");

    let journal = run_gameday(
        &gameday,
        &experiments,
        &executor_arc,
        &controls_arc,
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
