//! Load-test lifecycle helpers for the experiment runner.
//!
//! Extracted from `run_experiment` so the orchestrator stays readable: these
//! functions manage the `resilience.load` span and the background load process,
//! enriching the span with result metrics when the load test is stopped.

use super::{LoadExecutor, LoadHandle, RunConfig, TRACER_NAME};
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
) -> (Option<opentelemetry::ContextGuard>, Option<LoadHandle>) {
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
                tracing::error!(error = %e, "failed to start load test");
                None
            }
        }
    } else {
        None
    };

    (load_span_guard, load_handle)
}

/// Stop the running load test, collect results, and enrich the current
/// `resilience.load` span with result metrics.
///
/// Must be called while the span guard returned by [`start_load`] is still
/// attached, so the enrichment lands on the load span.
pub(super) fn stop_load(
    load_handle: Option<LoadHandle>,
    load_executor: Option<&std::sync::Arc<dyn LoadExecutor>>,
) -> Option<LoadResult> {
    if let (Some(handle), Some(load_exec)) = (load_handle, load_executor) {
        match load_exec.stop(handle) {
            Ok(result) => {
                // Enrich the resilience.load span with result metrics
                let span_cx = opentelemetry::Context::current();
                let span = span_cx.span();
                span.set_attribute(KeyValue::new(
                    "resilience.load.throughput_rps",
                    result.throughput_rps,
                ));
                span.set_attribute(KeyValue::new(
                    "resilience.load.latency_p50_ms",
                    result.latency_p50_ms,
                ));
                span.set_attribute(KeyValue::new(
                    "resilience.load.latency_p95_ms",
                    result.latency_p95_ms,
                ));
                span.set_attribute(KeyValue::new(
                    "resilience.load.latency_p99_ms",
                    result.latency_p99_ms,
                ));
                span.set_attribute(KeyValue::new(
                    "resilience.load.error_rate",
                    result.error_rate,
                ));
                span.set_attribute(KeyValue::new(
                    "resilience.load.total_requests",
                    i64::try_from(result.total_requests).unwrap_or(i64::MAX),
                ));
                span.set_attribute(KeyValue::new(
                    "resilience.load.thresholds_met",
                    result.thresholds_met,
                ));
                span.set_attribute(KeyValue::new(
                    "resilience.load.duration_s",
                    result.duration_s,
                ));

                tracing::info!(
                    throughput_rps = result.throughput_rps,
                    latency_p95_ms = result.latency_p95_ms,
                    error_rate = result.error_rate,
                    "load test completed"
                );
                Some(result)
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to collect load test results");
                None
            }
        }
    } else {
        None
    }
}
