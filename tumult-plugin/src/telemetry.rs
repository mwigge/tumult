//! `OTel` instrumentation for script plugin execution.

use opentelemetry::metrics::Meter;
use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};
use tumult_otel::SpanGuard;

const TRACER: &str = "tumult-plugin";

pub(crate) fn begin_execute(script_path: &str, timeout_s: Option<f64>) -> SpanGuard {
    let tracer = global::tracer(TRACER);
    let mut attrs = vec![KeyValue::new("script.path", script_path.to_string())];
    if let Some(t) = timeout_s {
        attrs.push(KeyValue::new("script.timeout_seconds", t));
    }
    let span = tracer
        .span_builder("script.execute")
        .with_kind(SpanKind::Internal)
        .with_attributes(attrs)
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    SpanGuard::new(cx.attach())
}

pub(crate) fn event_script_started(script_path: &str) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "script.started",
        vec![KeyValue::new("script.path", script_path.to_string())],
    );
}

pub(crate) fn event_script_completed(exit_code: i32, stdout_bytes: usize, stderr_bytes: usize) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "script.completed",
        vec![
            KeyValue::new("script.exit_code", i64::from(exit_code)),
            KeyValue::new(
                "script.stdout_bytes",
                i64::try_from(stdout_bytes).unwrap_or(i64::MAX),
            ),
            KeyValue::new(
                "script.stderr_bytes",
                i64::try_from(stderr_bytes).unwrap_or(i64::MAX),
            ),
        ],
    );
}

pub(crate) fn event_script_timed_out(script_path: &str, timeout_s: f64) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "script.timed_out",
        vec![
            KeyValue::new("script.path", script_path.to_string()),
            KeyValue::new("script.timeout_seconds", timeout_s),
        ],
    );
}

/// Record script execution counter.
pub(crate) fn record_execution(success: bool) {
    record_execution_on(&global::meter(TRACER), success);
}

/// [`record_execution`] against an explicit meter, so the recorded value can
/// be read back without a process-wide meter provider.
fn record_execution_on(meter: &Meter, success: bool) {
    let counter = meter
        .u64_counter("tumult.plugin.script.executions.total")
        .build();
    counter.add(1, &[KeyValue::new("script.success", success)]);
}

#[cfg(test)]
mod tests {
    use tumult_test_utils::{capture_spans, MetricCapture, MetricPoint};

    use super::*;

    #[test]
    fn execute_span_carries_path_timeout_and_lifecycle_events() {
        let ((), spans) = capture_spans(|| {
            let _span = begin_execute("/usr/local/bin/stress.sh", Some(30.0));
            event_script_started("/usr/local/bin/stress.sh");
            event_script_completed(3, 256, 17);
        });

        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.name, "script.execute");
        assert_eq!(span.span_kind, SpanKind::Internal);
        assert_eq!(
            span.attributes,
            [
                KeyValue::new("script.path", "/usr/local/bin/stress.sh"),
                KeyValue::new("script.timeout_seconds", 30.0),
            ]
        );
        let events = &span.events.events;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].name, "script.started");
        assert_eq!(
            events[0].attributes,
            [KeyValue::new("script.path", "/usr/local/bin/stress.sh")]
        );
        assert_eq!(events[1].name, "script.completed");
        assert_eq!(
            events[1].attributes,
            [
                KeyValue::new("script.exit_code", 3_i64),
                KeyValue::new("script.stdout_bytes", 256_i64),
                KeyValue::new("script.stderr_bytes", 17_i64),
            ]
        );
    }

    #[test]
    fn execute_span_without_timeout_omits_it_and_records_a_timeout_event() {
        let ((), spans) = capture_spans(|| {
            let _span = begin_execute("long-running.sh", None);
            event_script_timed_out("long-running.sh", 5.0);
        });

        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].attributes,
            [KeyValue::new("script.path", "long-running.sh")]
        );
        let events = &spans[0].events.events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "script.timed_out");
        assert_eq!(
            events[0].attributes,
            [
                KeyValue::new("script.path", "long-running.sh"),
                KeyValue::new("script.timeout_seconds", 5.0),
            ]
        );
    }

    #[test]
    fn execution_counter_counts_successes_and_failures_separately() {
        let capture = MetricCapture::new();
        let meter = capture.meter(TRACER);
        record_execution_on(&meter, true);
        record_execution_on(&meter, true);
        record_execution_on(&meter, false);

        let mut points = capture.points("tumult.plugin.script.executions.total");
        points.sort_by(|a, b| a.value.total_cmp(&b.value));
        assert_eq!(
            points,
            [
                MetricPoint {
                    value: 1.0,
                    attributes: vec![KeyValue::new("script.success", false)],
                },
                MetricPoint {
                    value: 2.0,
                    attributes: vec![KeyValue::new("script.success", true)],
                },
            ]
        );
    }
}
