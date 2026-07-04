//! `tumult new` (fault picker + template instantiation) and `tumult templates`.
//!
//! `new` has two modes:
//! - `--from <template>`: non-interactive; instantiate a curated starter,
//!   apply `--set key=value` overrides, validate, and write.
//! - no flags: an interactive picker (domain → action → args → target → probe
//!   → title). In a non-TTY it does not hang — it prints how to use `--from`
//!   or flags and exits non-zero.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use tumult_authoring::builder::{build_experiment_toon, ProbeSpec, ScaffoldRequest};
use tumult_authoring::catalog::{build_catalog, ActionKind, CatalogAction, FaultCatalog};
use tumult_authoring::templates::{all_templates, find_template, parse_overrides};

use indexmap::IndexMap;

// ── tumult templates ──────────────────────────────────────────

/// List the curated starter templates: name, description, and parameters.
///
/// # Errors
///
/// Never fails; returns `Result` for dispatch-signature uniformity.
#[must_use = "callers must handle the result"]
pub fn cmd_templates() -> Result<()> {
    let templates = all_templates();
    println!("Curated starter templates ({}):\n", templates.len());
    for t in templates {
        println!("  {}  [{}]", t.name, t.domain.label());
        println!("    {}", t.description);
        let params: Vec<String> = t
            .params
            .iter()
            .map(|p| format!("{}={}", p.name, p.default))
            .collect();
        if !params.is_empty() {
            println!("    params: {}", params.join(", "));
        }
        println!();
    }
    println!("Instantiate one with:");
    println!("  tumult new --from <name> [--set key=value ...] [--out <path>]");
    Ok(())
}

// ── tumult new ────────────────────────────────────────────────

/// Entry point for `tumult new`. Dispatches to the template path when `--from`
/// is given, otherwise runs the interactive picker.
///
/// # Errors
///
/// Returns an error if instantiation, validation, or writing fails, or (in the
/// interactive path) if there is no TTY to prompt on.
#[must_use = "callers must handle the result"]
pub fn cmd_new(from: Option<&str>, sets: &[String], out: Option<&Path>) -> Result<()> {
    match from {
        Some(template) => cmd_new_from_template(template, sets, out),
        None => cmd_new_interactive(out),
    }
}

/// Non-interactive: instantiate a curated starter, apply overrides, write.
fn cmd_new_from_template(name: &str, sets: &[String], out: Option<&Path>) -> Result<()> {
    let Some(template) = find_template(name) else {
        let available: Vec<&str> = all_templates().iter().map(|t| t.name).collect();
        bail!(
            "unknown template '{name}'. Available: {}\nRun `tumult templates` for details.",
            available.join(", ")
        );
    };

    let overrides = parse_overrides(sets).map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let overrides_map: std::collections::HashMap<String, String> = overrides.into_iter().collect();
    let toon = template
        .instantiate_toon(&overrides_map)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let out_path = out.map_or_else(|| PathBuf::from(format!("{name}.toon")), Path::to_path_buf);
    write_experiment(&out_path, &toon)?;
    Ok(())
}

/// Interactive picker. Degrades gracefully with no TTY.
fn cmd_new_interactive(out: Option<&Path>) -> Result<()> {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        bail!(
            "`tumult new` is interactive and needs a terminal.\n\
             In a non-interactive context, use a curated starter instead:\n\
             \n\
             \x20 tumult new --from <template> [--set key=value ...] [--out <path>]\n\
             \n\
             List templates with `tumult templates`."
        );
    }

    let catalog = build_catalog().unwrap_or_else(|_| FaultCatalog { domains: vec![] });
    if catalog.is_empty() {
        bail!(
            "no plugins were discovered, so there is no fault catalog to pick from.\n\
             Run `tumult new` from a directory with a ./plugins folder, or use a\n\
             curated starter: `tumult new --from <template>` (see `tumult templates`)."
        );
    }

    // 1. Domain.
    println!("Pick a fault domain:");
    for (i, d) in catalog.domains.iter().enumerate() {
        println!("  {}) {} ({} actions)", i + 1, d.label, d.actions.len());
    }
    let domain_idx = prompt_index("Domain", catalog.domains.len())?;
    let domain = &catalog.domains[domain_idx];

    // 2. Action (fault actions only, not probes).
    let actions: Vec<&CatalogAction> = domain
        .actions
        .iter()
        .filter(|a| a.kind == ActionKind::Action)
        .collect();
    if actions.is_empty() {
        bail!("domain '{}' has no fault actions to pick", domain.label);
    }
    println!("\nPick an action in {}:", domain.label);
    for (i, a) in actions.iter().enumerate() {
        println!("  {}) {} — {}", i + 1, a.name, a.description);
    }
    let action_idx = prompt_index("Action", actions.len())?;
    let action = actions[action_idx];

    // 3. Required arguments.
    let mut args: IndexMap<String, String> = IndexMap::new();
    let required: Vec<_> = action.args.iter().filter(|a| a.required).collect();
    if !required.is_empty() {
        println!("\nRequired arguments for {}:", action.name);
        for arg in required {
            let value = prompt_line(&format!("  {} ({})", arg.name, arg.description))?;
            if !value.is_empty() {
                args.insert(arg.name.clone(), value);
            }
        }
    }

    // 4. Target.
    let target = prompt_default("\nTarget (host / container / service)", "demo-target")?;

    // 5. Steady-state probe.
    let probe_cmd = prompt_default(
        "Steady-state probe command (health check)",
        &format!("echo \"{target} steady-state ok\""),
    )?;
    let probe_expect = prompt_default("Probe expected-output regex", "steady-state ok")?;
    let probe = ProbeSpec::Exec {
        command: probe_cmd,
        expect: probe_expect,
    };

    // 6. Title.
    let title = prompt_default("Experiment title", &format!("{} — {}", action.name, target))?;

    let request = ScaffoldRequest {
        title,
        plugin: action.plugin.clone(),
        action: action.name.clone(),
        args,
        target,
        probe,
    };
    let toon = build_experiment_toon(&request).map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let out_path = out.map_or_else(
        || PathBuf::from(format!("{}.toon", action.name)),
        Path::to_path_buf,
    );
    write_experiment(&out_path, &toon)?;
    Ok(())
}

/// Write the generated experiment, refusing to clobber an existing file, and
/// print the run command.
fn write_experiment(path: &Path, toon: &str) -> Result<()> {
    if path.exists() {
        bail!(
            "{} already exists — choose a different --out path",
            path.display()
        );
    }
    std::fs::write(path, toon).with_context(|| format!("failed to write {}", path.display()))?;
    println!("\nWrote validated experiment to {}", path.display());
    println!("Run it with:");
    println!("  tumult run {}", path.display());
    Ok(())
}

// ── prompt helpers (std-only; no external prompting crate) ─────

/// Read a trimmed line from stdin. EOF (Ctrl-D) aborts.
fn prompt_line(label: &str) -> Result<String> {
    print!("{label}: ");
    std::io::stdout().flush().ok();
    let mut buf = String::new();
    let n = std::io::stdin()
        .read_line(&mut buf)
        .context("failed to read from stdin")?;
    if n == 0 {
        bail!("aborted (end of input)");
    }
    Ok(buf.trim().to_string())
}

/// Prompt with a default applied when the user just presses Enter.
fn prompt_default(label: &str, default: &str) -> Result<String> {
    let value = prompt_line(&format!("{label} [{default}]"))?;
    Ok(if value.is_empty() {
        default.to_string()
    } else {
        value
    })
}

/// Prompt for a 1-based selection in `1..=len`, returning the 0-based index.
fn prompt_index(label: &str, len: usize) -> Result<usize> {
    loop {
        let raw = prompt_line(&format!("{label} (1-{len})"))?;
        match raw.parse::<usize>() {
            Ok(n) if n >= 1 && n <= len => return Ok(n - 1),
            _ => println!("Please enter a number between 1 and {len}."),
        }
    }
}
