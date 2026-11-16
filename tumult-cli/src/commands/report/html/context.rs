//! What a run was: its summary, the halt that ended it, and what it was
//! planned with.

use tumult_core::types::{HypothesisResult, Journal, NfrState, ObjectiveOutcome, TargetVersion};

use super::fmt::{
    fact, facts, millis, number, offset, optional, percent, seconds, section, table, text, tr, utc,
    yes_no,
};

/// The guard that halted the run and what it saw.
pub(super) fn halt(journal: &Journal) -> String {
    let Some(halt) = &journal.halt else {
        return String::new();
    };
    let observed = halt
        .observed
        .as_deref()
        .map_or_else(|| "no output".to_string(), text);
    let rows = [
        fact("Guard", &text(&halt.guard_name)),
        fact("Safe condition", &text(&halt.safe_condition)),
        fact("Observed", &observed),
        fact("Breaches", &halt.breach_count.to_string()),
        fact(
            "Breached at",
            &offset(halt.breached_at_ns, journal.started_at_ns),
        ),
        fact("Time to halt", &millis(halt.time_to_halt_ms)),
        fact("Rollback time", &millis(halt.rollback_ms)),
    ]
    .concat();
    format!(
        "<section id=\"halt\" class=\"callout\">\n<h2>Halted by a guard</h2>\n{}</section>\n",
        facts(&rows)
    )
}

/// The run at a glance.
pub(super) fn summary(journal: &Journal) -> String {
    let objectives = journal.objectives.as_ref().and_then(objectives_met);
    let cards = [
        card("Kind", &journal.kind.to_string()),
        card("Started", &utc(journal.started_at_ns)),
        card("Duration", &millis(journal.duration_ms)),
        optional_card(
            "Target version",
            journal.target_version.as_ref().map(version),
        ),
        card(
            "Steady state before",
            steady(journal.steady_state_before.as_ref()),
        ),
        card(
            "Steady state after",
            steady(journal.steady_state_after.as_ref()),
        ),
        optional_card("Objectives", objectives),
        card("Rollbacks", &rollbacks(journal)),
    ]
    .concat();
    format!("<section id=\"summary\" class=\"cards\">\n{cards}</section>\n")
}

fn card(label: &str, html: &str) -> String {
    format!(
        "<div class=\"card\"><div class=\"label\">{label}</div><div class=\"value\">{html}</div></div>\n"
    )
}

fn optional_card(label: &str, html: Option<String>) -> String {
    html.map_or_else(String::new, |html| card(label, &html))
}

fn steady(check: Option<&HypothesisResult>) -> &'static str {
    match check.map(|check| check.met) {
        Some(true) => "met",
        Some(false) => "not met",
        None => "not checked",
    }
}

/// The version of the target and where it came from. A version read from
/// telemetry that named several says which share of the requests it served.
fn version(version: &TargetVersion) -> String {
    let share = version.share().map_or_else(String::new, |share| {
        format!(", {} of the requests", percent(share))
    });
    format!(
        "{} ({}{share})",
        text(&version.version),
        version.version_source
    )
}

fn objectives_met(outcome: &ObjectiveOutcome) -> Option<String> {
    let all = outcome.verdicts.len();
    let met = outcome
        .verdicts
        .iter()
        .filter(|verdict| verdict.state == NfrState::Met)
        .count();
    (all > 0).then(|| format!("{met} of {all} met"))
}

fn rollbacks(journal: &Journal) -> String {
    if journal.rollback_results.is_empty() {
        return "none run".to_string();
    }
    format!(
        "{} run, {} failed",
        journal.rollback_results.len(),
        journal.rollback_failures
    )
}

/// The versions the telemetry of the target named inside the run, when it
/// named more than one.
pub(super) fn versions(journal: &Journal) -> String {
    let Some(version) = &journal.target_version else {
        return String::new();
    };
    let rows: String = version
        .shares
        .iter()
        .map(|share| tr(false, &[text(&share.version), share.requests.to_string()]))
        .collect();
    section(
        "target-versions",
        "Versions of the target",
        &table(&["Version", "Requests"], &rows),
    )
}

/// How far the faults of the run were allowed to reach, and how far they
/// did.
pub(super) fn blast_radius(journal: &Journal) -> String {
    let Some(radius) = &journal.blast_radius else {
        return String::new();
    };
    let cap = radius.max_concurrent_faults.map(|cap| cap.to_string());
    let rows = [
        optional("Note", radius.note.as_deref().map(text)),
        optional("Cap on concurrent faults", cap),
        fact(
            "Peak concurrent faults",
            &radius.peak_concurrent_faults.to_string(),
        ),
    ]
    .concat();
    section("blast-radius", "Blast radius", &facts(&rows))
}

/// What the run was expected to do, as its definition said before it ran.
pub(super) fn estimate(journal: &Journal) -> String {
    let Some(estimate) = &journal.estimate else {
        return String::new();
    };
    let rows = [
        fact("Expected outcome", &estimate.expected_outcome.to_string()),
        optional(
            "Expected recovery",
            estimate.expected_recovery_s.map(seconds),
        ),
        optional(
            "Expected degradation",
            estimate
                .expected_degradation
                .as_ref()
                .map(ToString::to_string),
        ),
        optional(
            "Expected data loss",
            estimate.expected_data_loss.map(yes_no),
        ),
        optional(
            "Confidence",
            estimate.confidence.as_ref().map(ToString::to_string),
        ),
        optional("Rationale", estimate.rationale.as_deref().map(text)),
        optional(
            "Prior runs",
            estimate.prior_runs.map(|runs| runs.to_string()),
        ),
    ]
    .concat();
    section("estimate", "Estimate", &facts(&rows))
}

/// How the run compares with its estimate.
pub(super) fn analysis(journal: &Journal) -> String {
    let Some(analysis) = &journal.analysis else {
        return String::new();
    };
    let delta = analysis
        .estimate_recovery_delta_s
        .map(|delta| format!("{} s", number(delta)));
    let rows = [
        optional("Estimate accuracy", analysis.estimate_accuracy.map(percent)),
        optional("Recovery against the estimate", delta),
        optional("Trend", analysis.trend.as_ref().map(ToString::to_string)),
        optional("Resilience score", analysis.resilience_score.map(number)),
    ]
    .concat();
    section("analysis", "Analysis", &facts(&rows))
}

/// The frameworks the run is evidence for, and the requirements it names.
pub(super) fn regulatory(journal: &Journal) -> String {
    let Some(mapping) = &journal.regulatory else {
        return String::new();
    };
    let frameworks: Vec<String> = mapping.frameworks.iter().map(|name| text(name)).collect();
    let named = if frameworks.is_empty() {
        String::new()
    } else {
        format!("<p>Frameworks: {}</p>\n", frameworks.join(", "))
    };
    let rows: String = mapping
        .requirements
        .iter()
        .map(|requirement| {
            tr(
                false,
                &[
                    text(&requirement.id),
                    text(&requirement.description),
                    text(&requirement.evidence),
                ],
            )
        })
        .collect();
    let requirements = table(&["Requirement", "Description", "Evidence"], &rows);
    section(
        "regulatory",
        "Regulatory mapping",
        &[named, requirements].concat(),
    )
}
