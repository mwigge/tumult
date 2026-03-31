//! OTel instrumentation for baseline acquisition.

use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};

const TRACER: &str = "tumult-baseline";

pub(crate) struct SpanGuard {
    _guard: opentelemetry::ContextGuard,
}

pub(crate) fn begin_acquire(probe_count: usize, method: &str) -> SpanGuard {
    let tracer = global::tracer(TRACER);
    let span = tracer
        .span_builder("baseline.acquire")
        .with_kind(SpanKind::Internal)
        .with_attributes(vec![
            KeyValue::new("baseline.probe_count", probe_count as i64),
            KeyValue::new("baseline.tolerance_method", method.to_string()),
        ])
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    SpanGuard {
        _guard: cx.attach(),
    }
}

pub(crate) fn event_tolerance_derived(lower: f64, upper: f64, total_samples: usize) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "baseline.tolerance.derived",
        vec![
            KeyValue::new("baseline.tolerance.lower", lower),
            KeyValue::new("baseline.tolerance.upper", upper),
            KeyValue::new("baseline.samples_total", total_samples as i64),
        ],
    );
}

pub(crate) fn event_anomaly_detected(reason: &str, cv: f64) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "baseline.anomaly.detected",
        vec![
            KeyValue::new("baseline.anomaly.reason", reason.to_string()),
            KeyValue::new("baseline.anomaly.cv", cv),
        ],
    );
}

pub(crate) fn record_baseline_gauges(
    probe_count: usize,
    samples_total: usize,
    tolerance_lower: f64,
    tolerance_upper: f64,
) {
    let meter = global::meter(TRACER);

    // probes_total and samples_total are monotonically increasing counters
    let c = meter
        .u64_counter("baseline.probes_total")
        .with_description("Total baseline probes executed")
        .build();
    c.add(probe_count as u64, &[]);

    let c = meter
        .u64_counter("baseline.samples_total")
        .with_description("Total baseline samples collected")
        .build();
    c.add(samples_total as u64, &[]);

    let g = meter.f64_gauge("baseline.tolerance.lower").build();
    g.record(tolerance_lower, &[]);

    let g = meter.f64_gauge("baseline.tolerance.upper").build();
    g.record(tolerance_upper, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_span_does_not_panic() {
        let _g = begin_acquire(3, "mean_stddev");
        event_tolerance_derived(10.0, 90.0, 100);
    }

    #[test]
    fn anomaly_event_does_not_panic() {
        let _g = begin_acquire(1, "percentile");
        event_anomaly_detected("high coefficient of variation", 0.75);
    }

    #[test]
    fn gauges_do_not_panic() {
        record_baseline_gauges(5, 500, 10.0, 90.0);
    }
}
