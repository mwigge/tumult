//! Experiment scaffolding (`init`) and dry-run rendering.

use std::path::Path;

use anyhow::{bail, Result};

use tumult_core::types::Experiment;

// ── Init command ──────────────────────────────────────────────

/// # Errors
///
/// Returns an error if the file already exists or cannot be written.
#[must_use = "callers must handle init errors"]
pub fn cmd_init(plugin: Option<&str>) -> Result<()> {
    init_at(Path::new("experiment.toon"), plugin)
}

pub(crate) fn init_at(path: &Path, plugin: Option<&str>) -> Result<()> {
    if path.exists() {
        bail!(
            "{} already exists — remove it first or use a different name",
            path.display()
        );
    }

    let template = generate_template(plugin);
    std::fs::write(path, &template)?;

    crate::outln!("Created {} from a template", path.display());
    if let Some(p) = plugin {
        crate::outln!(
            "Template references the {p} plugin — edit the method steps to use its actions"
        );
    }
    crate::outln!("Edit the file to configure your experiment, then run:");
    crate::outln!("  tumult run {}", path.display());

    Ok(())
}

pub(crate) fn generate_template(plugin: Option<&str>) -> String {
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

pub(crate) fn print_dry_run(experiment: &Experiment) {
    crate::outln!("=== DRY RUN ===\n");
    crate::outln!("Experiment: {}", experiment.title);
    if let Some(ref desc) = experiment.description {
        crate::outln!("Description: {desc}");
    }
    crate::outln!();

    if let Some(ref estimate) = experiment.estimate {
        crate::outln!("Phase 0 — Estimate:");
        crate::outln!("  Expected outcome: {:?}", estimate.expected_outcome);
        if let Some(recovery) = estimate.expected_recovery_s {
            crate::outln!("  Expected recovery: {recovery}s");
        }
        crate::outln!();
    }

    if let Some(ref baseline) = experiment.baseline {
        crate::outln!("Phase 1 — Baseline:");
        crate::outln!("  Duration: {}s", baseline.duration_s);
        crate::outln!("  Interval: {}s", baseline.interval_s);
        crate::outln!("  Method: {:?}", baseline.method);
        crate::outln!();
    }

    if let Some(ref hypothesis) = experiment.steady_state_hypothesis {
        crate::outln!("Hypothesis: {}", hypothesis.title);
        for probe in &hypothesis.probes {
            crate::outln!("  Probe: {}", probe.name);
        }
        crate::outln!();
    }

    crate::outln!("Phase 2 — Method ({} steps):", experiment.method.len());
    for (i, activity) in experiment.method.iter().enumerate() {
        let bg = if activity.background {
            " [background]"
        } else {
            ""
        };
        crate::outln!(
            "  {}. {} ({:?}){}",
            i + 1,
            activity.name,
            activity.activity_type,
            bg
        );
    }
    crate::outln!();

    if !experiment.rollbacks.is_empty() {
        crate::outln!("Rollbacks ({} steps):", experiment.rollbacks.len());
        for activity in &experiment.rollbacks {
            crate::outln!("  - {} ({:?})", activity.name, activity.activity_type);
        }
        crate::outln!();
    }

    if let Some(ref regulatory) = experiment.regulatory {
        crate::outln!("Regulatory: {}", regulatory.frameworks.join(", "));
    }

    crate::outln!("=== END DRY RUN ===");
}
