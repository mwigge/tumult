//! Experiment scaffolding (`init`) and dry-run rendering.

use std::path::Path;

use anyhow::{bail, Result};

use tumult_authoring::{build_catalog, encode_experiment};
use tumult_core::types::{Experiment, ExperimentTarget, LoadConfig, LoadProfile};

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

    let experiment = match plugin {
        None => tumult_authoring::starter()?,
        Some(plugin) => tumult_authoring::starter_for(&build_catalog()?, plugin)?,
    };
    std::fs::write(path, encode_experiment(&experiment)?)?;

    crate::outln!("Created {} from the starter template", path.display());
    if let Some(p) = plugin {
        crate::outln!(
            "The first step reads {p}::{}. Set its arguments, and add a fault of {p}.",
            experiment.method[0].name
        );
    }
    crate::outln!("Edit the file to configure your experiment, then run:");
    crate::outln!("  tumult run {}", path.display());

    Ok(())
}

// ── Dry run ───────────────────────────────────────────────────

/// What the run is judged by: its kind, target and recovery targets, then
/// the objectives and the load, each as its own paragraph.
fn judged_by_lines(experiment: &Experiment) -> Vec<String> {
    let mut lines = vec![format!("Kind: {}", experiment.kind)];
    lines.extend(experiment.target.as_ref().map(target_line));
    lines.extend(recovery_targets_line(experiment));
    if !experiment.nfr.is_empty() {
        lines.push(String::new());
        lines.push(format!("Objectives ({}):", experiment.nfr.len()));
    }
    for o in &experiment.nfr {
        lines.push(format!(
            "  - {} ({}, {}): {} {} {}",
            o.id, o.kind, o.criticality, o.metric, o.operator, o.threshold
        ));
    }
    if let Some(load) = &experiment.load {
        lines.push(String::new());
        lines.push(load_line(load));
    }
    lines
}

fn target_line(target: &ExperimentTarget) -> String {
    let details: Vec<String> = [
        ("technology", &target.technology),
        ("environment", &target.environment),
        ("criticality", &target.criticality),
        ("namespace", &target.namespace),
        ("version", &target.version),
    ]
    .into_iter()
    .filter_map(|(label, value)| value.as_ref().map(|v| format!("{label}: {v}")))
    .collect();
    if details.is_empty() {
        return format!("Target: {}", target.system);
    }
    format!("Target: {} ({})", target.system, details.join(", "))
}

fn recovery_targets_line(experiment: &Experiment) -> Option<String> {
    let targets: Vec<String> = [
        ("RTO", experiment.rto_target_s),
        ("RPO", experiment.rpo_target_s),
    ]
    .into_iter()
    .filter_map(|(label, seconds)| seconds.map(|s| format!("{label} {s}s")))
    .collect();
    (!targets.is_empty()).then(|| format!("Recovery targets: {}", targets.join(", ")))
}

/// `count` of a thing, with the word for `one` of it or for `many`.
fn counted<N>(count: N, one: &str, many: &str) -> String
where
    N: Copy + std::fmt::Display + PartialEq + From<u8>,
{
    let word = if count == N::from(1) { one } else { many };
    format!("{count} {word}")
}

fn load_line(load: &LoadConfig) -> String {
    let shape = match load.profile {
        None => "flat",
        Some(LoadProfile::Ramp { .. }) => "ramp",
        Some(LoadProfile::Spike { .. }) => "spike",
        Some(LoadProfile::Soak { .. }) => "soak",
    };
    let users = load.vus.map(|vus| counted(vus, "user", "users"));
    let duration = load.duration_s.map(|s| format!("{s}s"));
    let parts: Vec<String> = users
        .into_iter()
        .chain(duration)
        .chain([shape.to_string()])
        .collect();
    format!(
        "Load: {} {} ({})",
        load.tool,
        load.script.display(),
        parts.join(", ")
    )
}

pub(crate) fn print_dry_run(experiment: &Experiment) {
    crate::outln!("=== DRY RUN ===\n");
    crate::outln!("Experiment: {}", experiment.title);
    if let Some(ref desc) = experiment.description {
        crate::outln!("Description: {desc}");
    }
    for line in judged_by_lines(experiment) {
        crate::outln!("{line}");
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
        crate::outln!("Baseline (declared, a run does not use it):");
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

    if !experiment.guards.is_empty() {
        crate::outln!("Guards ({}):", experiment.guards.len());
        for guard in &experiment.guards {
            crate::outln!(
                "  - {}: probe {}, halts after {}",
                guard.name,
                guard.probe.name,
                counted(guard.min_breaches, "breach", "breaches")
            );
        }
        crate::outln!();
    }

    crate::outln!(
        "Phase 2 — Method ({}):",
        counted(experiment.method.len(), "step", "steps")
    );
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
        crate::outln!(
            "Rollbacks ({}):",
            counted(experiment.rollbacks.len(), "step", "steps")
        );
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
