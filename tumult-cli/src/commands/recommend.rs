//! `tumult recommend` orchestration (heuristics + optional agent-CLI
//! enhancement) and the `tumult agents` adapter-detection table.
//!
//! Agent-proposed experiments pass a validation gate before touching disk
//! (`tumult_intelligence::write_validated_experiments`): each `toon` block
//! is parsed and validated; only valid experiments are written, and
//! rejections are reported explicitly in the summary.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

use tumult_agent_cli::AdapterRegistry;
use tumult_intelligence::{
    json_with_agent, render_text_with_agent, write_validated_experiments, AgentOptions,
    OutputFormat, RecommendOptions, WriteOutcome,
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
        Some(dir) => write_validated_experiments(dir, &enhancement.experiments)?,
        None => WriteOutcome::default(),
    };

    match options.format {
        OutputFormat::Text => Ok(render_text_with_agent(
            &base,
            &enhancement,
            args.generate_dir.is_some().then_some(&outcome),
        )),
        OutputFormat::Json => {
            let value = json_with_agent(&heuristic, &enhancement, &outcome)?;
            serde_json::to_string_pretty(&value).context("encode JSON")
        }
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
