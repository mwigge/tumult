//! What a run says about itself on stdout when it has ended: whether its
//! steady state held, how its objectives came out, and what its load
//! measured.

use tumult_core::types::{HypothesisResult, Journal, LoadResult, NfrState, NfrVerdict};

use super::report::html::fmt::{latency, number, percent, rate};
use super::report::html::measures::must_be;

/// The lines about the run of `journal`. A run without a steady state,
/// objectives or a load has no line about it.
pub(super) fn of_run(journal: &Journal) -> String {
    let objectives = journal.objectives.as_ref();
    [
        hypothesis(
            journal.steady_state_before.as_ref(),
            journal.steady_state_after.as_ref(),
        ),
        objectives.map_or_else(String::new, |outcome| verdicts(&outcome.verdicts)),
        journal.load_result.as_ref().map_or_else(String::new, load),
    ]
    .concat()
}

fn hypothesis(before: Option<&HypothesisResult>, after: Option<&HypothesisResult>) -> String {
    if before.is_none() && after.is_none() {
        return String::new();
    }
    format!(
        "Hypothesis: {} before, {} after\n",
        held(before),
        held(after)
    )
}

/// Whether the steady state held when the run checked it.
fn held(result: Option<&HypothesisResult>) -> &'static str {
    match result.map(|result| result.met) {
        Some(true) => "met",
        Some(false) => "not met",
        None => "not checked",
    }
}

/// How many objectives ended in each state, and a line for each one that
/// was not met.
fn verdicts(verdicts: &[NfrVerdict]) -> String {
    let count = |state| verdicts.iter().filter(|v| v.state == state).count();
    let counts = format!(
        "Objectives: {} met, {} breached, {} inconclusive\n",
        count(NfrState::Met),
        count(NfrState::Breached),
        count(NfrState::Inconclusive)
    );
    let open: String = verdicts
        .iter()
        .filter(|verdict| verdict.state != NfrState::Met)
        .map(not_met)
        .collect();
    counts + &open
}

fn not_met(verdict: &NfrVerdict) -> String {
    let observed = verdict.observed.map_or_else(
        || "not measured".to_string(),
        |value| format!("observed {}", number(value)),
    );
    format!(
        "  {}: {}: {} must be {} {}, {observed}\n",
        verdict.state,
        verdict.nfr_id,
        verdict.metric,
        must_be(verdict.operator),
        number(verdict.threshold)
    )
}

/// The line about a load: what it measured in all, and whether it was cut.
pub(super) fn load(load: &LoadResult) -> String {
    let thresholds = if load.thresholds_met {
        "met"
    } else {
        "not met"
    };
    let cut = if load.execution.aborted {
        ", cut before its planned end"
    } else {
        ""
    };
    format!(
        "Load ({}): {} requests, {}, p50 {}, p95 {}, p99 {}, error rate {}, \
         thresholds {thresholds}{cut}\n",
        load.tool,
        load.total_requests,
        rate(load.throughput_rps),
        latency(load.latency_p50_ms),
        latency(load.latency_p95_ms),
        latency(load.latency_p99_ms),
        percent(load.error_rate)
    )
}
