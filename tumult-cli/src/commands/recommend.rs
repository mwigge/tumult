//! `tumult recommend` orchestration (heuristics + optional agent-CLI
//! enhancement) and the `tumult agents` adapter-detection table.
//!
//! Agent-proposed experiments pass a validation gate before touching disk:
//! each `toon` block is parsed (`parse_experiment`) and validated
//! (`validate_experiment`); only valid experiments are written, and
//! rejections are reported honestly in the summary.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use tumult_agent_cli::AdapterRegistry;
use tumult_core::engine::{parse_experiment, validate_experiment};
use tumult_intelligence::{
    AgentEnhancement, AgentOptions, OutputFormat, RecommendOptions, RecommendationOutput,
};

/// Parsed `--agent*` / `--generate-experiments` flags of `tumult recommend`.
#[derive(Debug, Clone)]
pub struct AgentArgs {
    /// Adapter name from [`AdapterRegistry::builtin`] (e.g. `claude-code`).
    pub agent: String,
    /// Optional model override passed to the agent CLI.
    pub model: Option<String>,
    /// Agent CLI timeout in seconds.
    pub timeout_secs: u64,
    /// Directory to write validated agent-proposed experiments into.
    pub generate_dir: Option<PathBuf>,
}

/// Run the recommendation flow: heuristics first, then (when `agent` is
/// given) agent enhancement, experiment validation, and file writing.
///
/// Returns the full rendered output (text or JSON per `options.format`).
///
/// # Errors
///
/// Returns an error for an unknown adapter name (listing the available
/// adapters), any agent CLI failure (missing binary, auth, timeout,
/// unparseable output), an unwritable output directory, or JSON encoding
/// failures.
pub fn cmd_recommend(options: &RecommendOptions, agent: Option<&AgentArgs>) -> Result<String> {
    let Some(args) = agent else {
        return tumult_intelligence::recommend(options);
    };

    let heuristic = tumult_intelligence::recommend_output(options);
    let base = tumult_intelligence::render(&heuristic, options.format)?;

    let registry = AdapterRegistry::builtin();
    let adapter = registry.get(&args.agent)?;
    let agent_options = AgentOptions {
        model: args.model.clone(),
        timeout: Duration::from_secs(args.timeout_secs),
        generate_experiments: args.generate_dir.is_some(),
        ..AgentOptions::default()
    };
    let enhancement = tumult_intelligence::enhance(&heuristic, adapter, &agent_options)?;

    let outcome = match args.generate_dir.as_deref() {
        Some(dir) => write_experiments(dir, &enhancement.experiments)?,
        None => WriteOutcome::default(),
    };

    match options.format {
        OutputFormat::Text => Ok(render_text_with_agent(
            &base,
            &enhancement,
            args.generate_dir.is_some().then_some(&outcome),
        )),
        OutputFormat::Json => render_json_with_agent(&heuristic, &enhancement, &outcome),
    }
}

/// Render the `tumult agents` table: every registered adapter probed for
/// install/version/auth state, with an install hint when missing.
#[must_use]
pub fn cmd_agents() -> String {
    let registry = AdapterRegistry::builtin();
    let mut output = String::new();
    writeln!(
        output,
        "{:<14} {:<10} {:<10} DETAIL",
        "ADAPTER", "INSTALLED", "VERSION"
    )
    .ok();
    for (name, probe) in registry.detect_all() {
        let installed = if probe.installed { "yes" } else { "no" };
        let version = probe.version.as_deref().unwrap_or("-");
        let mut detail = probe.detail.trim().to_string();
        if !probe.installed {
            if let Ok(adapter) = registry.get(name) {
                let hint = adapter.install_hint();
                if !detail.contains(hint) {
                    write!(detail, " Install with: {hint}").ok();
                }
            }
        }
        writeln!(output, "{name:<14} {installed:<10} {version:<10} {detail}").ok();
    }
    output
}

/// Result of gating agent-proposed experiments through validation.
#[derive(Debug, Default)]
struct WriteOutcome {
    /// Paths of experiments that parsed, validated, and were written.
    written: Vec<PathBuf>,
    /// Parse/validation error per rejected (unwritten) experiment.
    rejected: Vec<String>,
}

fn write_experiments(dir: &Path, blocks: &[String]) -> Result<WriteOutcome> {
    let mut outcome = WriteOutcome::default();
    if !blocks.is_empty() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("create experiment output dir {}", dir.display()))?;
    }
    for block in blocks {
        let experiment =
            parse_experiment(block).and_then(|exp| validate_experiment(&exp).map(|()| exp));
        match experiment {
            Ok(experiment) => {
                let path = unique_path(dir, &slugify(&experiment.title));
                std::fs::write(&path, block)
                    .with_context(|| format!("write experiment {}", path.display()))?;
                outcome.written.push(path);
            }
            Err(err) => outcome.rejected.push(err.to_string()),
        }
    }
    Ok(outcome)
}

/// Sanitize an experiment title into a filesystem-safe slug.
fn slugify(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "experiment".to_string()
    } else {
        slug.to_string()
    }
}

/// `<dir>/<slug>.toon`, never overwriting: on collision append `-2`, `-3`, ...
fn unique_path(dir: &Path, slug: &str) -> PathBuf {
    let candidate = dir.join(format!("{slug}.toon"));
    if !candidate.exists() {
        return candidate;
    }
    (2..=u32::MAX)
        .map(|n| dir.join(format!("{slug}-{n}.toon")))
        .find(|path| !path.exists())
        .unwrap_or(candidate)
}

fn render_text_with_agent(
    base: &str,
    enhancement: &AgentEnhancement,
    outcome: Option<&WriteOutcome>,
) -> String {
    let mut text = base.trim_end().to_string();
    text.push_str("\n\n=== Agent-enhanced recommendations (");
    text.push_str(&enhancement.adapter);
    text.push_str(") ===\n");
    if let Some(model) = &enhancement.model {
        writeln!(text, "Model: {model}").ok();
    }
    writeln!(text).ok();
    writeln!(text, "{}", enhancement.recommendations.trim_end()).ok();

    if let Some(outcome) = outcome {
        writeln!(text).ok();
        for path in &outcome.written {
            writeln!(text, "Wrote {}", path.display()).ok();
        }
        for error in &outcome.rejected {
            writeln!(text, "Rejected experiment: {error}").ok();
        }
        writeln!(
            text,
            "{} experiment(s) written, {} rejected (validation failed)",
            outcome.written.len(),
            outcome.rejected.len()
        )
        .ok();
    }
    text
}

fn render_json_with_agent(
    heuristic: &RecommendationOutput,
    enhancement: &AgentEnhancement,
    outcome: &WriteOutcome,
) -> Result<String> {
    let mut value = serde_json::to_value(heuristic).context("encode JSON")?;
    value["agent"] = serde_json::json!({
        "adapter": enhancement.adapter,
        "model": enhancement.model,
        "recommendations": enhancement.recommendations,
        "experiments_written": outcome
            .written
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        "experiments_rejected": outcome
            .rejected
            .iter()
            .map(|error| serde_json::json!({ "error": error }))
            .collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&value).context("encode JSON")
}

#[cfg(test)]
mod slug_tests {
    use super::{slugify, unique_path};

    #[test]
    fn slugify_sanitizes_titles() {
        assert_eq!(
            slugify("Redis resilience — verify recovery!"),
            "redis-resilience-verify-recovery"
        );
        assert_eq!(slugify("  ***  "), "experiment");
        assert_eq!(slugify("UPPER case 123"), "upper-case-123");
    }

    #[test]
    fn unique_path_appends_counter_on_collision() {
        let dir = tempfile::tempdir().expect("tempdir");
        let first = unique_path(dir.path(), "demo");
        assert_eq!(first, dir.path().join("demo.toon"));
        std::fs::write(&first, "x").expect("write");
        let second = unique_path(dir.path(), "demo");
        assert_eq!(second, dir.path().join("demo-2.toon"));
        std::fs::write(&second, "x").expect("write");
        assert_eq!(
            unique_path(dir.path(), "demo"),
            dir.path().join("demo-3.toon")
        );
    }
}
