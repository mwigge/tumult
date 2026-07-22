//! Low-level telemetry and time helpers shared across runner submodules:
//! span attribute extraction, span status, trace/span id capture, the
//! epoch-nanosecond clock, and interrupted-journal construction.

use crate::types::{
    Activity, ActivityType, Experiment, ExperimentStatus, Journal, Provider, SpanId, TraceId,
};

use opentelemetry::trace::TraceContextExt;
use opentelemetry::KeyValue;

/// Build a Journal for an experiment interrupted before it started.
pub(crate) fn make_interrupted_journal(experiment: &Experiment, now_ns: i64) -> Journal {
    Journal::for_experiment(
        experiment,
        uuid::Uuid::new_v4().to_string(),
        ExperimentStatus::Interrupted,
        now_ns,
    )
}

/// Extract target attributes from an activity's provider.
pub(crate) fn target_attributes(activity: &Activity) -> Vec<KeyValue> {
    match &activity.provider {
        Provider::Process { path, .. } => vec![
            KeyValue::new("resilience.target.type", "process"),
            KeyValue::new("resilience.target.name", path.clone()),
        ],
        Provider::Native {
            plugin, function, ..
        } => vec![
            KeyValue::new("resilience.target.type", "native"),
            KeyValue::new("resilience.target.name", plugin.clone()),
            KeyValue::new(
                "resilience.target.component",
                format!("{plugin}::{function}"),
            ),
        ],
        Provider::Script {
            plugin, function, ..
        } => vec![
            KeyValue::new("resilience.target.type", "script"),
            KeyValue::new("resilience.target.name", plugin.clone()),
            KeyValue::new(
                "resilience.target.component",
                format!("{plugin}::{function}"),
            ),
        ],
    }
}

/// Extract fault attributes from an activity.
pub(crate) fn fault_attributes(activity: &Activity) -> Vec<KeyValue> {
    let fault_type = match activity.activity_type {
        ActivityType::Action => "injection",
        ActivityType::Probe => "observation",
    };
    vec![
        KeyValue::new("resilience.fault.type", fault_type),
        KeyValue::new("resilience.fault.name", activity.name.clone()),
    ]
}

/// Set span error status if the outcome failed.
pub(crate) fn set_span_status_from_outcome(success: bool, error: Option<&str>) {
    if !success {
        let ctx = opentelemetry::Context::current();
        let span = ctx.span();
        let desc = error.unwrap_or("activity failed");
        span.set_status(opentelemetry::trace::Status::error(desc.to_string()));
    }
}

/// Get the current trace ID from the active span context.
pub(crate) fn current_trace_id() -> TraceId {
    let ctx = opentelemetry::Context::current();
    let sc = ctx.span().span_context().clone();
    if sc.is_valid() {
        TraceId(sc.trace_id().to_string())
    } else {
        TraceId::empty()
    }
}

/// Get the current span ID from the active span context.
pub(crate) fn current_span_id() -> SpanId {
    let ctx = opentelemetry::Context::current();
    let sc = ctx.span().span_context().clone();
    if sc.is_valid() {
        SpanId(sc.span_id().to_string())
    } else {
        SpanId::empty()
    }
}

/// Gets current time as epoch nanoseconds.
///
/// # Panics
///
/// Panics if the system clock is outside the representable i64 nanosecond range
/// (before 1677 or after 2262).
#[must_use]
pub fn epoch_nanos_now() -> i64 {
    chrono::Utc::now()
        .timestamp_nanos_opt()
        .expect("timestamp overflow: clock outside i64 nanosecond range")
}
