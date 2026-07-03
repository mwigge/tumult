//! `GameDay` tools: run, analyze, and list multi-experiment `GameDay` sessions.

use std::fmt::Write as _;
use std::path::Path;

use crate::error::ToolError;
use crate::tools::listing::extract_title;

/// Runs a `GameDay` — all experiments under shared load.
///
/// # Errors
///
/// Returns a [`ToolError`] if the `GameDay` cannot be read, parsed,
/// or any experiment fails to execute.
#[allow(clippy::too_many_lines)] // GameDay orchestration spans load setup, multi-experiment execution, and result aggregation
pub fn gameday_run(gameday_path: &str) -> Result<String, ToolError> {
    use tumult_core::controls::ControlRegistry;
    use tumult_core::engine::parse_experiment;
    use tumult_core::runner::{run_gameday, RunConfig};
    use tumult_core::types::GameDay;

    let path = Path::new(gameday_path);
    let content = std::fs::read_to_string(path)?;

    let gameday: GameDay = toon_format::decode_default(&content)
        .map_err(|e| ToolError::Parse(format!("failed to parse gameday: {e}")))?;

    let gameday_dir = path.parent().unwrap_or(Path::new("."));

    let mut experiments = Vec::new();
    for gd_exp in &gameday.experiments {
        let exp_path = if gd_exp.path.is_absolute() {
            gd_exp.path.clone()
        } else {
            gameday_dir.join(&gd_exp.path)
        };
        let exp_content = std::fs::read_to_string(&exp_path)?;
        let experiment = parse_experiment(&exp_content).map_err(|e| {
            ToolError::Parse(format!("failed to parse {}: {e}", exp_path.display()))
        })?;
        experiments.push(experiment);
    }

    let executor: std::sync::Arc<dyn tumult_core::runner::ActivityExecutor> =
        std::sync::Arc::new(crate::handler::ProcessExecutor);
    let controls = std::sync::Arc::new(ControlRegistry::new());
    let config = RunConfig::default();

    let journal = run_gameday(&gameday, &experiments, &executor, &controls, &config)
        .map_err(|e| ToolError::Execution(format!("gameday failed: {e}")))?;

    // Write journal
    let journal_path = path.with_extension("journal.toon");
    let toon_out = toon_format::encode_default(&journal)
        .map_err(|e| ToolError::Execution(format!("failed to encode journal: {e}")))?;
    std::fs::write(&journal_path, &toon_out)?;

    let mut output = String::new();
    writeln!(output, "GameDay: {}", journal.title).ok();
    writeln!(output, "Status: {}", journal.compliance_status).ok();
    writeln!(output, "Duration: {:.1}s", journal.duration_s).ok();
    writeln!(
        output,
        "Resilience Score: {:.2}",
        journal.resilience_score.overall
    )
    .ok();
    writeln!(
        output,
        "Experiments: {}/{} passed",
        journal
            .experiment_journals
            .iter()
            .filter(|j| j.status == tumult_core::types::ExperimentStatus::Completed)
            .count(),
        journal.experiment_journals.len()
    )
    .ok();
    writeln!(output, "Journal: {}", journal_path.display()).ok();

    Ok(output)
}

/// Analyzes a completed `GameDay` journal.
///
/// # Errors
///
/// Returns a [`ToolError`] if the journal cannot be read or parsed.
pub fn gameday_analyze(gameday_path: &str) -> Result<String, ToolError> {
    use tumult_core::types::GameDayJournal;

    let path = Path::new(gameday_path);
    let journal_path = path.with_extension("journal.toon");
    let content = std::fs::read_to_string(&journal_path)?;

    let journal: GameDayJournal = toon_format::decode_default(&content)
        .map_err(|e| ToolError::Parse(format!("failed to parse: {e}")))?;

    let mut output = String::new();
    writeln!(output, "GameDay: {}", journal.title).ok();
    writeln!(output, "Status: {}", journal.compliance_status).ok();
    writeln!(output, "Duration: {:.1}s", journal.duration_s).ok();
    writeln!(output, "Score: {:.2}", journal.resilience_score.overall).ok();
    writeln!(
        output,
        "  Pass rate: {:.2}",
        journal.resilience_score.pass_rate
    )
    .ok();
    writeln!(
        output,
        "  Recovery: {:.2}",
        journal.resilience_score.recovery_compliance
    )
    .ok();
    writeln!(
        output,
        "  Load impact: {:.2}",
        journal.resilience_score.load_impact_tolerance
    )
    .ok();
    writeln!(
        output,
        "  Compliance: {:.2}",
        journal.resilience_score.compliance_coverage
    )
    .ok();

    for (i, ej) in journal.experiment_journals.iter().enumerate() {
        let icon = if ej.status == tumult_core::types::ExperimentStatus::Completed {
            "PASS"
        } else {
            "FAIL"
        };
        writeln!(
            output,
            "  #{} [{}] {} ({}ms)",
            i + 1,
            icon,
            ej.experiment_title,
            ej.duration_ms
        )
        .ok();
    }

    Ok(output)
}

/// Lists `.gameday.toon` files found recursively under `search_root`.
///
/// # Errors
///
/// Returns [`ToolError::InvalidInput`] if `search_root` is not a directory.
pub fn gameday_list(search_root: &str) -> Result<String, ToolError> {
    let root = Path::new(search_root);
    if !root.is_dir() {
        return Err(ToolError::InvalidInput(format!(
            "not a directory: {search_root}"
        )));
    }

    let mut entries = Vec::new();
    collect_gameday_files(root, &mut entries);

    if entries.is_empty() {
        return Ok("No .gameday.toon files found.".to_string());
    }

    let mut output = String::new();
    for (path, title) in &entries {
        writeln!(output, "{title}  ({path})").ok();
    }
    Ok(output)
}

fn collect_gameday_files(dir: &Path, entries: &mut Vec<(String, String)>) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_gameday_files(&path, entries);
        } else if path.extension().and_then(|e| e.to_str()) == Some("toon")
            && path
                .file_stem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.ends_with(".gameday"))
        {
            let title = std::fs::read_to_string(&path)
                .ok()
                .and_then(|c| extract_title(&c))
                .unwrap_or_else(|| "(untitled)".to_string());
            entries.push((path.display().to_string(), title));
        }
    }
}
