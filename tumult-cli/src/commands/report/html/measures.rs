//! What a run measured: its objectives, its load, and its probes during
//! and after the fault.

use tumult_core::types::{
    DuringResult, Journal, LoadExecution, LoadResult, LoadSample, NfrOperator, NfrState,
    NfrVerdict, ObjectiveOutcome, PostResult, ProbeDuring, ProbePost, RequestCounts,
};

use super::charts;
use super::fmt::{
    badge, fact, facts, latency, number, offset, optional, percent, rate, seconds, section, table,
    text, tr, yes_no,
};

/// The recovery objectives against what was measured, and the verdict on
/// each objective of the run.
pub(super) fn objectives(journal: &Journal) -> String {
    let Some(outcome) = &journal.objectives else {
        return String::new();
    };
    let body = [
        recovery(outcome),
        charts::headroom(&outcome.verdicts),
        verdicts(&outcome.verdicts),
    ]
    .concat();
    section("objectives", "Objectives", &body)
}

fn recovery(outcome: &ObjectiveOutcome) -> String {
    let objectives = [
        ("RTO", outcome.rto_target_s, outcome.rto_measured_s),
        ("RPO", outcome.rpo_target_s, outcome.rpo_measured_s),
    ];
    let rows: String = objectives
        .iter()
        .filter(|(_, target, measured)| target.is_some() || measured.is_some())
        .map(|(name, target, measured)| {
            let target = target.map_or_else(|| "not set".to_string(), seconds);
            let measured = measured.map_or_else(|| "not measured".to_string(), seconds);
            tr(false, &[(*name).to_string(), target, measured])
        })
        .collect();
    table(&["Recovery", "Target", "Measured"], &rows)
}

fn verdicts(verdicts: &[NfrVerdict]) -> String {
    let rows: String = verdicts.iter().map(verdict_row).collect();
    let head = [
        "Objective",
        "Kind",
        "Metric",
        "Must be",
        "Observed",
        "Margin",
        "State",
        "Criticality",
    ];
    table(&head, &rows)
}

/// How a value must compare to its threshold, in words.
pub(in crate::commands) fn must_be(operator: NfrOperator) -> &'static str {
    match operator {
        NfrOperator::Lte => "at most",
        NfrOperator::Gte => "at least",
        NfrOperator::Eq => "equal to",
    }
}

/// What a verdict observed, in words; `None` when it observed nothing. A
/// run that did not recover observed the time it watched: a lower bound.
pub(in crate::commands) fn observed(verdict: &NfrVerdict) -> Option<String> {
    let value = number(verdict.observed?);
    if verdict.recovered == Some(false) {
        return Some(format!("at least {value}, not recovered"));
    }
    Some(value)
}

fn verdict_row(verdict: &NfrVerdict) -> String {
    let class = match verdict.state {
        NfrState::Met => "ok",
        NfrState::Breached => "fail",
        NfrState::Inconclusive => "warn",
    };
    tr(
        verdict.state == NfrState::Breached,
        &[
            text(&verdict.nfr_id),
            verdict.kind.to_string(),
            text(&verdict.metric),
            format!(
                "{} {}",
                must_be(verdict.operator),
                number(verdict.threshold)
            ),
            observed(verdict).unwrap_or_else(|| "not measured".to_string()),
            verdict.margin.map_or_else(String::new, number),
            badge(class, &verdict.state.to_string()),
            verdict.criticality.to_string(),
        ],
    )
}

/// The load of the run: what it measured in all, how its plan was carried
/// out, and each of its windows.
pub(super) fn load(journal: &Journal) -> String {
    let Some(load) = &journal.load_result else {
        return String::new();
    };
    let rows = [totals(load), execution(&load.execution)].concat();
    let body = [
        facts(&rows),
        charts::load(journal, load),
        windows(load, journal.started_at_ns),
    ]
    .concat();
    section("load", "Load", &body)
}

fn totals(load: &LoadResult) -> String {
    let thresholds = if load.thresholds_met {
        "met"
    } else {
        "not met"
    };
    [
        fact("Tool", &load.tool.to_string()),
        fact("Users", &load.vus.to_string()),
        fact("Duration", &seconds(load.duration_s)),
        fact("Requests", &load.total_requests.to_string()),
        fact("Throughput", &rate(load.throughput_rps)),
        fact("Latency p50", &latency(load.latency_p50_ms)),
        fact("Latency p95", &latency(load.latency_p95_ms)),
        fact("Latency p99", &latency(load.latency_p99_ms)),
        fact("Error rate", &percent(load.error_rate)),
        fact("Thresholds", thresholds),
    ]
    .concat()
}

/// How the plan of the load was carried out. A row for what there is to
/// say, none for a count of zero.
fn execution(execution: &LoadExecution) -> String {
    let windows = execution
        .planned_windows
        .zip(execution.completed_windows)
        .map(|(planned, completed)| format!("{completed} of {planned} completed"));
    let cut = execution
        .aborted
        .then(|| "the load was cut before its planned end".to_string());
    let reasons: Vec<String> = execution.reasons.iter().map(|why| text(why)).collect();
    [
        optional("Windows", windows),
        optional(
            "Windows without an answer",
            counted(execution.unanswered_windows),
        ),
        optional(
            "Windows that never ran in a blackout",
            counted(execution.lost_to_unanswered),
        ),
        optional(
            "Windows with a p99 among the timeouts",
            counted(execution.censored_windows),
        ),
        optional("Cut", cut),
        optional(
            "Why the plan fell short",
            Some(reasons.join("; ")).filter(|why| !why.is_empty()),
        ),
        optional(
            "Requests by outcome",
            execution.requests.as_ref().map(outcomes),
        ),
    ]
    .concat()
}

fn counted(count: u32) -> Option<String> {
    (count > 0).then(|| count.to_string())
}

/// How many requests were sent, and how many ended in each way that any
/// did.
fn outcomes(counts: &RequestCounts) -> String {
    let ended = [
        ("ok", counts.ok),
        ("timed out", counts.timed_out),
        ("transport errors", counts.transport),
        ("4xx", counts.http_4xx),
        ("5xx", counts.http_5xx),
        ("other status", counts.http_other),
        ("unclassified", counts.unclassified),
    ];
    let mut parts = vec![format!("sent {}", counts.sent)];
    parts.extend(
        ended
            .iter()
            .filter(|(_, count)| *count > 0)
            .map(|(how, count)| format!("{how} {count}")),
    );
    parts.join(", ")
}

fn windows(load: &LoadResult, run_started_ns: i64) -> String {
    let rows: String = load
        .samples
        .iter()
        .map(|window| window_row(window, run_started_ns))
        .collect();
    let head = [
        "Start",
        "Length",
        "Users",
        "Requests",
        "Throughput",
        "p50",
        "p95",
        "p99",
        "Error rate",
    ];
    table(&head, &rows)
}

fn window_row(window: &LoadSample, run_started_ns: i64) -> String {
    tr(
        false,
        &[
            offset(window.ts_ns, run_started_ns),
            seconds(window.duration_s),
            window.vus.to_string(),
            window.requests.to_string(),
            rate(window.throughput_rps),
            percentile(window, window.latency_p50_ms),
            percentile(window, window.latency_p95_ms),
            percentile(window, window.latency_p99_ms),
            percent(window.error_rate),
        ],
    )
}

/// A percentile of a window. One that fell among the requests that timed
/// out is a lower bound, and says so.
fn percentile(window: &LoadSample, latency_ms: f64) -> String {
    if window.censors(latency_ms) {
        return format!("at least {}", latency(latency_ms));
    }
    latency(latency_ms)
}

/// What the probes of the steady state read while the fault was active and
/// after it.
pub(super) fn measurements(journal: &Journal) -> String {
    let body = [
        Some(charts::probes(journal)),
        journal.during_result.as_ref().map(during),
        journal.post_result.as_ref().map(post),
    ]
    .into_iter()
    .flatten()
    .collect::<String>();
    section("measurements", "Measurements", &body)
}

fn during(during: &DuringResult) -> String {
    let rows = [
        fact("Fault active", &seconds(during.fault_active_s)),
        fact("Sample interval", &seconds(during.sample_interval_s)),
    ]
    .concat();
    let probes: String = during
        .probes
        .iter()
        .map(|probe| during_row(probe, during.started_at_ns))
        .collect();
    let head = [
        "Probe",
        "Samples",
        "Mean",
        "Min",
        "Max",
        "Error rate",
        "Breaches",
        "First breach",
    ];
    format!(
        "<h3>During the fault</h3>\n{}{}",
        facts(&rows),
        table(&head, &probes)
    )
}

/// The row of one probe. Its first breach is given from the start of the
/// fault.
fn during_row(probe: &ProbeDuring, fault_started_ns: i64) -> String {
    let first_breach = probe
        .breached_at_ns
        .map_or_else(String::new, |at| offset(at, fault_started_ns));
    tr(
        false,
        &[
            text(&probe.name),
            probe.samples.to_string(),
            latency(probe.mean),
            latency(probe.min),
            latency(probe.max),
            percent(probe.error_rate),
            probe.breach_count.to_string(),
            first_breach,
        ],
    )
}

fn post(post: &PostResult) -> String {
    let rows = [
        fact("Watched for", &seconds(post.duration_s)),
        fact("Samples", &post.samples.to_string()),
        fact(
            "Recovery time",
            &recovery_time(post.recovery_time_s, post.full_recovery),
        ),
        fact("Full recovery", &yes_no(post.full_recovery)),
        optional("MTTR", post.mttr_s.map(seconds)),
    ]
    .concat();
    let probes: String = post.probes.iter().map(post_row).collect();
    let head = [
        "Probe",
        "Mean",
        "p95",
        "Error rate",
        "Back in tolerance",
        "Recovery time",
    ];
    format!(
        "<h3>After the fault</h3>\n{}{}",
        facts(&rows),
        table(&head, &probes)
    )
}

fn post_row(probe: &ProbePost) -> String {
    tr(
        false,
        &[
            text(&probe.name),
            latency(probe.mean),
            latency(probe.p95),
            percent(probe.error_rate),
            yes_no(probe.returned_to_baseline),
            recovery_time(probe.recovery_time_s, probe.returned_to_baseline),
        ],
    )
}

/// A recovery time. What did not come back was still failing at the end of
/// the time measured, so that time is a lower bound, and says so.
fn recovery_time(time_s: f64, recovered: bool) -> String {
    if recovered {
        return seconds(time_s);
    }
    format!("at least {}", seconds(time_s))
}
