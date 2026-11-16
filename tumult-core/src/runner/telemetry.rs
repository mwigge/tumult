//! Low-level telemetry and time helpers shared across runner submodules:
//! span attribute extraction, span status, trace/span id capture, the
//! epoch-nanosecond clock, and interrupted-journal construction.

use crate::types::{
    Activity, ActivityType, Experiment, ExperimentStatus, Journal, Provider, SpanId, TargetVersion,
    TraceId,
};

use opentelemetry::trace::{Status, TraceContextExt};
use opentelemetry::KeyValue;
use tumult_otel::attributes;

/// Build a Journal for an experiment interrupted before it started.
pub(crate) fn make_interrupted_journal(experiment: &Experiment, now_ns: i64) -> Journal {
    Journal::for_experiment(
        experiment,
        uuid::Uuid::new_v4().to_string(),
        ExperimentStatus::Interrupted,
        now_ns,
    )
}

/// `resilience.target.*` attributes for the experiment root span, from the
/// definition's `target` block. Empty when the definition declares none.
pub(crate) fn experiment_target_attributes(experiment: &Experiment) -> Vec<KeyValue> {
    let Some(target) = &experiment.target else {
        return Vec::new();
    };
    let mut attrs = vec![KeyValue::new(
        "resilience.target.system",
        target.system.clone(),
    )];
    if let Some(t) = &target.technology {
        attrs.push(KeyValue::new("resilience.target.technology", t.clone()));
    }
    if let Some(e) = &target.environment {
        attrs.push(KeyValue::new("resilience.target.environment", e.clone()));
    }
    if let Some(c) = &target.criticality {
        attrs.push(KeyValue::new("resilience.target.criticality", c.clone()));
    }
    if let Some(version) = TargetVersion::declared(experiment) {
        attrs.extend(target_version_attributes(&version));
    }
    attrs
}

/// The version a run measured and where it came from, for the root span.
pub(crate) fn target_version_attributes(version: &TargetVersion) -> [KeyValue; 2] {
    [
        KeyValue::new("resilience.target.version", version.version.clone()),
        KeyValue::new(
            "resilience.target.version_source",
            version.version_source.to_string(),
        ),
    ]
}

/// How a run ended, for its root span: the status in the spelling of the
/// journal, the verdict on the hypothesis and the recovery time when the
/// run has them, and the blast radius the definition declares.
pub(crate) fn outcome_attributes(journal: &Journal) -> Vec<KeyValue> {
    let mut attrs = vec![KeyValue::new(
        attributes::OUTCOME,
        journal.status.to_string(),
    )];
    if let Some(met) = journal.hypothesis_met() {
        attrs.push(KeyValue::new(attributes::HYPOTHESIS_MET, met));
    }
    if let Some(seconds) = journal.recovery_time_s() {
        attrs.push(KeyValue::new(attributes::RECOVERY_TIME_S, seconds));
    }
    let declared = journal.blast_radius.as_ref().and_then(|b| b.note.clone());
    if let Some(note) = declared {
        attrs.push(KeyValue::new(attributes::FAULT_BLAST_RADIUS, note));
    }
    attrs
}

/// The status of the root span: Ok for a completed run, and an error that
/// names the status for every other ending.
pub(crate) fn outcome_span_status(status: &ExperimentStatus) -> Status {
    if *status == ExperimentStatus::Completed {
        return Status::Ok;
    }
    Status::error(status.to_string())
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

/// Derive the plugin name an activity is served by, for metric labels.
///
/// Native and script activities name their plugin explicitly; process
/// activities are served by the built-in executor, so the provider type is
/// the plugin.
pub(crate) fn plugin_name(activity: &Activity) -> String {
    match &activity.provider {
        Provider::Native { plugin, .. } | Provider::Script { plugin, .. } => plugin.clone(),
        Provider::Process { .. } => "process".to_string(),
    }
}

/// Set span error status if the outcome failed.
pub(crate) fn set_span_status_from_outcome(success: bool, error: Option<&str>) {
    if !success {
        let ctx = opentelemetry::Context::current();
        let span = ctx.span();
        let desc = error.unwrap_or("activity failed");
        span.set_status(Status::error(desc.to_string()));
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
