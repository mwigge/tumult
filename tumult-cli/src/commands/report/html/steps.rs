//! The steady state of a run and its steps.

use tumult_core::types::{ActivityResult, ActivityStatus, HypothesisResult, Journal};

use super::fmt::{badge, millis, offset, section, table, text, tr};

/// The steady state before and after the method, with what each probe
/// read.
pub(super) fn steady_state(journal: &Journal, trace_ui_base: Option<&str>) -> String {
    let checks = [
        ("Before the method", &journal.steady_state_before),
        ("After the method", &journal.steady_state_after),
    ];
    let body: String = checks
        .iter()
        .filter_map(|(when, check)| Some(hypothesis(when, check.as_ref()?, trace_ui_base)))
        .collect();
    section("steady-state", "Steady state", &body)
}

fn hypothesis(when: &str, check: &HypothesisResult, trace_ui_base: Option<&str>) -> String {
    let met = if check.met {
        badge("ok", "met")
    } else {
        badge("fail", "not met")
    };
    let rows: String = check
        .probe_results
        .iter()
        .map(|probe| probe_row(probe, trace_ui_base))
        .collect();
    let probes = table(&["Probe", "Status", "Duration", "Trace", "Output"], &rows);
    format!("<h3>{when}: {} {met}</h3>\n{probes}", text(&check.title))
}

fn probe_row(probe: &ActivityResult, trace_ui_base: Option<&str>) -> String {
    tr(
        failed(probe),
        &[
            text(&probe.name),
            status(&probe.status),
            millis(probe.duration_ms),
            trace(probe, trace_ui_base),
            detail(probe),
        ],
    )
}

/// The steps of the method and the rollbacks, in the order they ran.
pub(super) fn steps(journal: &Journal, trace_ui_base: Option<&str>) -> String {
    let phases = [
        ("method", &journal.method_results),
        ("rollback", &journal.rollback_results),
    ];
    let rows: String = phases
        .iter()
        .flat_map(|(phase, results)| results.iter().map(move |step| (phase, step)))
        .map(|(phase, step)| step_row(phase, step, journal.started_at_ns, trace_ui_base))
        .collect();
    let head = [
        "Phase", "Step", "Type", "Status", "Start", "Duration", "Trace", "Detail",
    ];
    section("steps", "Steps", &table(&head, &rows))
}

/// The row of one step. Its start is given from the start of the run.
pub(crate) fn step_row(
    phase: &str,
    step: &ActivityResult,
    run_started_ns: i64,
    trace_ui_base: Option<&str>,
) -> String {
    tr(
        failed(step),
        &[
            phase.to_string(),
            text(&step.name),
            step.activity_type.to_string(),
            status(&step.status),
            offset(step.started_at_ns, run_started_ns),
            millis(step.duration_ms),
            trace(step, trace_ui_base),
            detail(step),
        ],
    )
}

fn failed(result: &ActivityResult) -> bool {
    matches!(
        result.status,
        ActivityStatus::Failed | ActivityStatus::Timeout
    )
}

fn status(status: &ActivityStatus) -> String {
    let class = match status {
        ActivityStatus::Succeeded => "ok",
        ActivityStatus::Failed | ActivityStatus::Timeout => "fail",
        ActivityStatus::Skipped => "muted",
    };
    badge(class, &status.to_string())
}

/// The trace of a step: the first 16 characters of its id, as a link to
/// the whole trace when the report was given a trace UI.
fn trace(result: &ActivityResult, trace_ui_base: Option<&str>) -> String {
    let id = result.trace_id.as_str();
    if id.is_empty() {
        return String::new();
    }
    let short = text(&id.chars().take(16).collect::<String>());
    let Some(base) = trace_ui_base else {
        return format!(r#"<span class="trace-link">{short}</span>"#);
    };
    format!(
        r#"<a class="trace-link" href="{}/trace/{}">{short}</a>"#,
        text(base.trim_end_matches('/')),
        text(id)
    )
}

/// What a step printed and the error it ended with. A step that failed and
/// left neither says so.
fn detail(result: &ActivityResult) -> String {
    let parts: Vec<String> = [
        ("detail-output", &result.output),
        ("detail-error", &result.error),
    ]
    .iter()
    .filter_map(|(class, said)| {
        let said = said.as_deref().filter(|said| !said.is_empty())?;
        Some(format!(r#"<span class="{class}">{}</span>"#, text(said)))
    })
    .collect();
    if parts.is_empty() && failed(result) {
        return r#"<span class="detail-error">(no detail captured)</span>"#.to_string();
    }
    parts.join(" ")
}
