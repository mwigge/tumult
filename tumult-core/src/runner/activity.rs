//! Activity execution: hypothesis evaluation, single/parallel activity
//! execution, and rollback handling.

use crate::controls::{ControlRegistry, LifecycleEvent};
use crate::engine::evaluate_tolerance;
use crate::execution::{
    make_result, partition_background, should_rollback, ResultParams, RollbackStrategy,
};
use crate::types::{
    Activity, ActivityResult, ActivityStatus, ActivityType, Experiment, Hypothesis,
    HypothesisResult, SpanId, TraceId,
};

use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::KeyValue;
use tokio_util::sync::CancellationToken;

use super::telemetry::{
    current_span_id, current_trace_id, epoch_nanos_now, fault_attributes,
    set_span_status_from_outcome, target_attributes,
};
use super::{ActivityExecutor, TRACER_NAME};

/// Evaluate a steady-state hypothesis by running its probes.
pub(crate) fn evaluate_hypothesis(
    hypothesis: &Hypothesis,
    executor: &dyn ActivityExecutor,
    controls: &ControlRegistry,
) -> HypothesisResult {
    let mut probe_results = Vec::with_capacity(hypothesis.probes.len());
    let mut all_met = true;

    for probe in &hypothesis.probes {
        let result = execute_single_activity(probe, executor, controls);

        // Check tolerance if defined
        if let Some(ref tolerance) = probe.tolerance {
            if let Some(ref output) = result.output {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(output) {
                    if !evaluate_tolerance(&value, tolerance) {
                        all_met = false;
                    }
                } else {
                    // If output isn't valid JSON, try as string
                    let value = serde_json::Value::String(output.clone());
                    if !evaluate_tolerance(&value, tolerance) {
                        all_met = false;
                    }
                }
            } else {
                // Tolerance defined but no output -- cannot evaluate, treat as failure
                all_met = false;
            }
        } else if result.status != ActivityStatus::Succeeded {
            all_met = false;
        }

        probe_results.push(result);
    }

    HypothesisResult {
        title: hypothesis.title.clone(),
        met: all_met,
        probe_results,
    }
}

/// Execute a single activity with `OTel` instrumentation.
///
/// Extracted so both foreground and background paths share the same logic.
fn execute_single_activity(
    activity: &Activity,
    executor: &dyn ActivityExecutor,
    controls: &ControlRegistry,
) -> ActivityResult {
    let tracer = opentelemetry::global::tracer(TRACER_NAME);

    controls.emit(&LifecycleEvent::BeforeActivity {
        name: activity.name.clone(),
    });

    let span_name = match activity.activity_type {
        ActivityType::Action => "resilience.action",
        ActivityType::Probe => "resilience.probe",
    };
    let mut attrs = vec![
        KeyValue::new("resilience.action.name", activity.name.clone()),
        KeyValue::new(
            "resilience.activity.type",
            activity.activity_type.to_string(),
        ),
    ];
    attrs.extend(target_attributes(activity));
    attrs.extend(fault_attributes(activity));
    let span = tracer
        .span_builder(span_name.to_string())
        .with_attributes(attrs)
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    let _guard = cx.attach();

    let started_at_ns = epoch_nanos_now();
    let outcome = executor.execute(activity);
    set_span_status_from_outcome(outcome.success, outcome.error.as_deref());

    let result = make_result(ResultParams {
        activity,
        started_at_ns,
        duration_ms: outcome.duration_ms,
        success: outcome.success,
        output: outcome.output,
        error: outcome.error,
        trace_id: current_trace_id(),
        span_id: current_span_id(),
    });

    controls.emit(&LifecycleEvent::AfterActivity {
        name: activity.name.clone(),
    });

    result
}

/// Execute a list of activities, partitioning into foreground (sequential)
/// and background (spawned concurrently via `JoinSet`).
///
/// Foreground activities execute sequentially with pause handling.
/// Background activities are spawned immediately and joined after all
/// foreground work completes.
///
/// If a cancellation token is provided and cancelled, stops executing
/// remaining foreground activities and returns results collected so far
/// (background tasks are still joined).
pub(crate) fn execute_activities(
    activities: &[Activity],
    executor: &(dyn ActivityExecutor + Sync),
    controls: &ControlRegistry,
    cancellation_token: Option<&CancellationToken>,
) -> Vec<ActivityResult> {
    let (foreground, background) = partition_background(activities);

    // Capacity: foreground results first, then background joined at end.
    let mut fg_results = Vec::with_capacity(foreground.len());

    // Spawn background activities on scoped OS threads *then* run foreground
    // sequentially inside the same scope.  `std::thread::scope` guarantees all
    // background threads are joined before the scope exits (i.e. after foreground
    // completes), giving us true concurrency without unsafe lifetime extension.
    let bg_results: Vec<std::result::Result<ActivityResult, _>> = std::thread::scope(|scope| {
        // 1. Spawn background threads immediately.
        let handles: Vec<_> = background
            .iter()
            .map(|&activity| {
                scope.spawn(move || execute_single_activity(activity, executor, controls))
            })
            .collect();

        // 2. Run foreground activities sequentially while background threads run.
        //    Note: pause_before_s / pause_after_s use std::thread::sleep here
        //    because we are inside a synchronous scope closure.  Background
        //    threads are already running concurrently so blocking the OS thread
        //    here is acceptable.
        for &activity in &foreground {
            // Check cancellation before each activity.
            if let Some(token) = cancellation_token {
                if token.is_cancelled() {
                    tracing::warn!(
                        activity = %activity.name,
                        "cancelled before activity execution"
                    );
                    break;
                }
            }

            if let Some(pause) = activity.pause_before_s {
                if pause > 0.0 {
                    opentelemetry::Context::current().span().add_event(
                        "experiment.pause.before",
                        vec![
                            KeyValue::new("activity.name", activity.name.clone()),
                            KeyValue::new("pause_seconds", pause),
                        ],
                    );
                    std::thread::sleep(std::time::Duration::from_secs_f64(pause));
                    opentelemetry::Context::current().span().add_event(
                        "experiment.resume.before",
                        vec![KeyValue::new("activity.name", activity.name.clone())],
                    );
                }
            }

            let result = execute_single_activity(activity, executor, controls);

            if let Some(pause) = activity.pause_after_s {
                if pause > 0.0 {
                    opentelemetry::Context::current().span().add_event(
                        "experiment.pause.after",
                        vec![
                            KeyValue::new("activity.name", activity.name.clone()),
                            KeyValue::new("pause_seconds", pause),
                        ],
                    );
                    std::thread::sleep(std::time::Duration::from_secs_f64(pause));
                    opentelemetry::Context::current().span().add_event(
                        "experiment.resume.after",
                        vec![KeyValue::new("activity.name", activity.name.clone())],
                    );
                }
            }

            fg_results.push(result);
        }

        // 3. Join background threads (scope exit would also do this, but collect
        //    the results explicitly so we can handle panics below).
        handles
            .into_iter()
            .map(std::thread::ScopedJoinHandle::join)
            .collect()
    });

    // Foreground results first, then background -- preserving the expected ordering
    // (foreground is the "primary" execution path; background runs alongside it).
    let mut results = fg_results;
    results.reserve(background.len());

    for (activity, join_result) in background.iter().zip(bg_results) {
        match join_result {
            Ok(activity_result) => results.push(activity_result),
            Err(_panic) => {
                tracing::error!(activity = %activity.name, "background activity panicked");
                results.push(ActivityResult {
                    name: activity.name.clone(),
                    activity_type: activity.activity_type.clone(),
                    status: ActivityStatus::Failed,
                    started_at_ns: epoch_nanos_now(),
                    duration_ms: 0,
                    output: None,
                    error: Some("background activity panicked".to_string()),
                    trace_id: TraceId::empty(),
                    span_id: SpanId::empty(),
                });
            }
        }
    }

    results
}

/// Run `experiment`'s rollback activities, if any, and if `strategy` calls
/// for rollbacks given `deviated`. Wraps execution in a `resilience.rollback`
/// span and the `BeforeRollback`/`AfterRollback` lifecycle events. Returns an
/// empty vec if rollbacks are skipped.
pub(crate) fn run_rollbacks(
    experiment: &Experiment,
    executor: &std::sync::Arc<dyn ActivityExecutor>,
    controls: &std::sync::Arc<ControlRegistry>,
    strategy: &RollbackStrategy,
    deviated: bool,
) -> Vec<ActivityResult> {
    if experiment.rollbacks.is_empty() || !should_rollback(strategy, deviated) {
        return vec![];
    }

    controls.emit(&LifecycleEvent::BeforeRollback);
    let rb_tracer = opentelemetry::global::tracer(TRACER_NAME);
    let rb_span = rb_tracer
        .span_builder("resilience.rollback")
        .start(&rb_tracer);
    let rb_cx = opentelemetry::Context::current_with_span(rb_span);
    let _rb_guard = rb_cx.attach();
    let results =
        execute_rollback_activities(&experiment.rollbacks, executor.as_ref(), controls.as_ref());
    controls.emit(&LifecycleEvent::AfterRollback);
    results
}

/// Execute rollback activities. Unlike `execute_activities`, this function
/// continues executing remaining rollbacks even if one fails, logging a
/// warning for each failure.
fn execute_rollback_activities(
    activities: &[Activity],
    executor: &dyn ActivityExecutor,
    controls: &ControlRegistry,
) -> Vec<ActivityResult> {
    activities
        .iter()
        .map(|activity| {
            let result = execute_single_activity(activity, executor, controls);
            if result.status == ActivityStatus::Failed {
                tracing::warn!(
                    activity = %activity.name,
                    error = ?result.error,
                    "rollback activity failed, continuing with remaining rollbacks"
                );
            }
            result
        })
        .collect()
}
