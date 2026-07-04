//! Validation gate and shared rendering for agent-proposed experiments.
//!
//! Agent-proposed experiments pass a validation gate before touching disk:
//! each `toon` block is parsed (`parse_experiment`) and validated
//! (`validate_experiment`); only valid experiments are written, and
//! rejections are reported honestly in the summary. Shared by the CLI's
//! `tumult recommend --agent` flow and the MCP `tumult_recommend` tool.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use tumult_core::engine::{parse_experiment, validate_experiment};

use crate::agent::AgentEnhancement;
use crate::types::RecommendationOutput;

/// Result of gating agent-proposed experiments through validation.
#[derive(Debug, Default)]
pub struct WriteOutcome {
    /// Paths of experiments that parsed, validated, and were written.
    pub written: Vec<PathBuf>,
    /// Parse/validation error per rejected (unwritten) experiment.
    pub rejected: Vec<String>,
}

/// Gate each proposed `toon` block through parse + validation and write the
/// valid ones into `dir` (created on demand) as `<title-slug>.toon`, never
/// overwriting existing files.
///
/// # Errors
///
/// Returns an error when the output directory cannot be created or a
/// validated experiment cannot be written. Invalid experiments are not
/// errors; they are reported via [`WriteOutcome::rejected`].
pub fn write_validated_experiments(dir: &Path, blocks: &[String]) -> Result<WriteOutcome> {
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

/// Append the agent enhancement (and, when experiments were gated, the
/// write/reject summary) to the base heuristic text rendering.
#[must_use]
pub fn render_text_with_agent(
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

/// The heuristic output plus an `agent` object (adapter, model, enhanced
/// recommendations, written/rejected experiments) as one JSON value — the
/// shape shared by `tumult recommend --format json` and the MCP tool's
/// structured content.
///
/// # Errors
///
/// Returns an error when the heuristic output cannot be encoded as JSON.
pub fn json_with_agent(
    heuristic: &RecommendationOutput,
    enhancement: &AgentEnhancement,
    outcome: &WriteOutcome,
) -> Result<serde_json::Value> {
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
    Ok(value)
}

#[cfg(test)]
mod tests {
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
