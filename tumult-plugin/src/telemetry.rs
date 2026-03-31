//! OTel instrumentation for script plugin execution.

use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};

const TRACER: &str = "tumult-plugin";

pub(crate) struct SpanGuard {
    _guard: opentelemetry::ContextGuard,
}

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
    SpanGuard {
        _guard: cx.attach(),
    }
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
            KeyValue::new("script.stdout_bytes", stdout_bytes as i64),
            KeyValue::new("script.stderr_bytes", stderr_bytes as i64),
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
    let meter = global::meter(TRACER);
    let counter = meter.u64_counter("script.executions_total").build();
    counter.add(1, &[KeyValue::new("script.success", success)]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_span_does_not_panic() {
        let _g = begin_execute("/usr/local/bin/stress.sh", Some(30.0));
        event_script_started("/usr/local/bin/stress.sh");
        event_script_completed(0, 256, 0);
    }

    #[test]
    fn timeout_event_does_not_panic() {
        let _g = begin_execute("long-running.sh", Some(5.0));
        event_script_timed_out("long-running.sh", 5.0);
    }

    #[test]
    fn counter_does_not_panic() {
        record_execution(true);
        record_execution(false);
    }
}
