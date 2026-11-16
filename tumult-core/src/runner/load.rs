//! Load-test lifecycle helpers for the experiment runner.
//!
//! Extracted from `run_experiment` so the orchestrator stays readable: these
//! functions manage the `resilience.load` span and the background load process,
//! enriching the span with result metrics when the load test is stopped.

use super::{LoadExecutor, LoadHandle, RunConfig, RunnerError, TRACER_NAME};
use crate::types::{Experiment, LoadResult};

use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::KeyValue;

/// Start the background load test (if configured) and open the
/// `resilience.load` span.
///
/// Returns the attached span guard (which must be dropped by the caller once
/// the load result has been collected) and the running load handle, if any.
pub(super) fn start_load(
    experiment: &Experiment,
    config: &RunConfig,
) -> Result<(Option<opentelemetry::ContextGuard>, Option<LoadHandle>), RunnerError> {
    let load_tracer = opentelemetry::global::tracer(TRACER_NAME);
    let load_span_guard = if let Some(ref load_config) = experiment.load {
        let tool_name = format!("{}", load_config.tool);
        let span = load_tracer
            .span_builder("resilience.load")
            .with_attributes(vec![
                KeyValue::new("resilience.load.tool", tool_name),
                KeyValue::new(
                    "resilience.load.vus",
                    i64::from(load_config.vus.unwrap_or(0)),
                ),
                KeyValue::new(
                    "resilience.load.script",
                    load_config.script.display().to_string(),
                ),
            ])
            .start(&load_tracer);
        let cx = opentelemetry::Context::current_with_span(span);
        Some(cx.attach())
    } else {
        None
    };

    let load_handle = if let (Some(ref load_config), Some(ref load_exec)) =
        (&experiment.load, &config.load_executor)
    {
        match load_exec.start(load_config) {
            Ok(handle) => {
                tracing::info!(
                    tool = %load_config.tool,
                    script = %load_config.script.display(),
                    "load test started"
                );
                Some(handle)
            }
            Err(e) => {
                return Err(RunnerError::LoadStart(e));
            }
        }
    } else {
        None
    };

    Ok((load_span_guard, load_handle))
}

/// How the load test is ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoadEnd {
    /// The method finished: let the tool wind down and report.
    Stop,
    /// The run was halted or interrupted: end the load now, before the
    /// rollbacks, keeping what had been measured.
    Abort,
}

/// Whether a declared load left evidence: a result with at least one
/// request. `true` when the experiment declares no load.
pub(super) fn load_has_evidence(experiment: &Experiment, result: Option<&LoadResult>) -> bool {
    experiment.load.is_none() || result.is_some_and(|r| r.total_requests > 0)
}

/// Whether the load missed its declared thresholds.
pub(super) fn load_missed_thresholds(result: Option<&LoadResult>) -> bool {
    result.is_some_and(|r| !r.thresholds_met)
}

/// End the running load test, collect its result, and put the outcome on
/// the current `resilience.load` span — the measurements when there is a
/// result, the error when there is none, and on either path whether the
/// load was aborted or its plan truncated.
///
/// Must be called while the span guard returned by [`start_load`] is still
/// attached, so the enrichment lands on the load span.
pub(super) fn stop_load(
    load_handle: Option<LoadHandle>,
    load_executor: Option<&std::sync::Arc<dyn LoadExecutor>>,
    end: LoadEnd,
) -> Option<LoadResult> {
    let (Some(handle), Some(load_exec)) = (load_handle, load_executor) else {
        return None;
    };
    let collected = match end {
        LoadEnd::Stop => load_exec.stop(handle),
        LoadEnd::Abort => load_exec.abort(handle),
    };
    let span_cx = opentelemetry::Context::current();
    let span = span_cx.span();
    match collected {
        Ok(result) => {
            for attribute in result_attributes(&result) {
                span.set_attribute(attribute);
            }
            if let Some(reason) = shortfall(&result) {
                span.set_status(opentelemetry::trace::Status::error(reason));
            }
            log_result(&result);
            Some(result)
        }
        Err(e) => {
            for attribute in failure_attributes(end, &e) {
                span.set_attribute(attribute);
            }
            span.set_status(opentelemetry::trace::Status::error(e.clone()));
            tracing::error!(
                error = %e,
                aborted = end == LoadEnd::Abort,
                "failed to collect load test results"
            );
            None
        }
    }
}

/// Why the load did not run as planned, when it did not.
fn shortfall(result: &LoadResult) -> Option<String> {
    let execution = &result.execution;
    if !(execution.aborted || execution.truncated()) {
        return None;
    }
    if execution.reasons.is_empty() {
        return Some("load did not run as planned".into());
    }
    Some(execution.reasons.join("; "))
}

/// The `resilience.load.*` attributes of a collected result.
pub(super) fn result_attributes(result: &LoadResult) -> Vec<KeyValue> {
    let execution = &result.execution;
    let mut attributes = vec![
        KeyValue::new("resilience.load.throughput_rps", result.throughput_rps),
        KeyValue::new("resilience.load.latency_p50_ms", result.latency_p50_ms),
        KeyValue::new("resilience.load.latency_p95_ms", result.latency_p95_ms),
        KeyValue::new("resilience.load.latency_p99_ms", result.latency_p99_ms),
        KeyValue::new("resilience.load.error_rate", result.error_rate),
        KeyValue::new(
            "resilience.load.total_requests",
            i64::try_from(result.total_requests).unwrap_or(i64::MAX),
        ),
        KeyValue::new("resilience.load.thresholds_met", result.thresholds_met),
        KeyValue::new("resilience.load.duration_s", result.duration_s),
        KeyValue::new("resilience.load.aborted", execution.aborted),
        KeyValue::new("resilience.load.truncated", execution.truncated()),
    ];
    if let Some(planned) = execution.planned_windows {
        attributes.push(KeyValue::new(
            "resilience.load.planned_windows",
            i64::from(planned),
        ));
    }
    if let Some(completed) = execution.completed_windows {
        attributes.push(KeyValue::new(
            "resilience.load.completed_windows",
            i64::from(completed),
        ));
        attributes.push(KeyValue::new(
            "resilience.load.unanswered_windows",
            i64::from(execution.unanswered_windows),
        ));
        attributes.push(KeyValue::new(
            "resilience.load.lost_to_unanswered",
            i64::from(execution.lost_to_unanswered),
        ));
    }
    if !execution.reasons.is_empty() {
        attributes.push(KeyValue::new(
            "resilience.load.reason",
            execution.reasons.join("; "),
        ));
    }
    attributes
}

/// The `resilience.load.*` attributes of a load that left no result.
pub(super) fn failure_attributes(end: LoadEnd, error: &str) -> Vec<KeyValue> {
    vec![
        KeyValue::new("resilience.load.error", error.to_string()),
        KeyValue::new("resilience.load.aborted", end == LoadEnd::Abort),
        KeyValue::new("resilience.load.thresholds_met", false),
    ]
}

fn log_result(result: &LoadResult) {
    let execution = &result.execution;
    if execution.aborted {
        tracing::warn!(
            total_requests = result.total_requests,
            planned_windows = execution.planned_windows,
            completed_windows = execution.completed_windows,
            reasons = ?execution.reasons,
            "load test aborted"
        );
        return;
    }
    if execution.truncated() {
        tracing::warn!(
            planned_windows = execution.planned_windows,
            completed_windows = execution.completed_windows,
            reasons = ?execution.reasons,
            "load plan truncated"
        );
        return;
    }
    tracing::info!(
        throughput_rps = result.throughput_rps,
        latency_p95_ms = result.latency_p95_ms,
        error_rate = result.error_rate,
        thresholds_met = result.thresholds_met,
        "load test completed"
    );
}
