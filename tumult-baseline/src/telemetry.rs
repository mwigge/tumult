//! `OTel` instrumentation for baseline acquisition.

use opentelemetry::metrics::Meter;
use opentelemetry::trace::{SpanKind, Status, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};
use tumult_otel::SpanGuard;

const TRACER: &str = "tumult-baseline";

pub(crate) fn begin_acquire(probe_count: usize, method: &str) -> SpanGuard {
    let tracer = global::tracer(TRACER);
    let span = tracer
        .span_builder("baseline.acquire")
        .with_kind(SpanKind::Internal)
        .with_attributes(vec![
            KeyValue::new(
                "baseline.probe_count",
                i64::try_from(probe_count).unwrap_or(i64::MAX),
            ),
            KeyValue::new("baseline.tolerance_method", method.to_string()),
        ])
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    SpanGuard::new(cx.attach())
}

pub(crate) fn event_tolerance_derived(lower: f64, upper: f64, total_samples: usize) {
    let cx = opentelemetry::Context::current();
    cx.span().add_event(
        "tolerance.derived",
        vec![
            KeyValue::new("baseline.tolerance.lower", lower),
            KeyValue::new("baseline.tolerance.upper", upper),
            KeyValue::new(
                "baseline.samples_total",
                i64::try_from(total_samples).unwrap_or(i64::MAX),
            ),
        ],
    );
}

pub(crate) fn event_anomaly_detected(reason: &str, cv: f64) {
    let cx = opentelemetry::Context::current();
    let span = cx.span();
    span.add_event(
        "anomaly.detected",
        vec![
            KeyValue::new("baseline.anomaly.reason", reason.to_string()),
            KeyValue::new("baseline.anomaly.cv", cv),
        ],
    );
    span.set_status(Status::error(reason.to_string()));
}

pub(crate) fn record_baseline_gauges(
    probe_count: usize,
    samples_total: usize,
    tolerance_lower: f64,
    tolerance_upper: f64,
) {
    record_baseline_gauges_on(
        &global::meter(TRACER),
        probe_count,
        samples_total,
        tolerance_lower,
        tolerance_upper,
    );
}

/// [`record_baseline_gauges`] against an explicit meter, so the recorded
/// values can be read back without a process-wide meter provider.
fn record_baseline_gauges_on(
    meter: &Meter,
    probe_count: usize,
    samples_total: usize,
    tolerance_lower: f64,
    tolerance_upper: f64,
) {
    // probes and samples are monotonically increasing counters
    let c = meter
        .u64_counter("tumult.baseline.probes.total")
        .with_description("Total baseline probes executed")
        .build();
    c.add(probe_count as u64, &[]);

    let c = meter
        .u64_counter("tumult.baseline.samples.total")
        .with_description("Total baseline samples collected")
        .build();
    c.add(samples_total as u64, &[]);

    let g = meter.f64_gauge("tumult.baseline.tolerance.lower").build();
    g.record(tolerance_lower, &[]);

    let g = meter.f64_gauge("tumult.baseline.tolerance.upper").build();
    g.record(tolerance_upper, &[]);
}

#[cfg(test)]
mod tests {
    use tumult_test_utils::{capture_spans, MetricCapture};

    use super::*;

    #[test]
    fn acquire_span_carries_probe_count_method_and_derived_tolerance() {
        let ((), spans) = capture_spans(|| {
            let _span = begin_acquire(3, "mean_stddev");
            event_tolerance_derived(10.0, 90.0, 100);
        });

        assert_eq!(spans.len(), 1);
        let span = &spans[0];
        assert_eq!(span.name, "baseline.acquire");
        assert_eq!(span.span_kind, SpanKind::Internal);
        assert_eq!(
            span.attributes,
            [
                KeyValue::new("baseline.probe_count", 3_i64),
                KeyValue::new("baseline.tolerance_method", "mean_stddev"),
            ]
        );
        assert_eq!(span.status, Status::Unset);
        let events = &span.events.events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "tolerance.derived");
        assert_eq!(
            events[0].attributes,
            [
                KeyValue::new("baseline.tolerance.lower", 10.0),
                KeyValue::new("baseline.tolerance.upper", 90.0),
                KeyValue::new("baseline.samples_total", 100_i64),
            ]
        );
    }

    #[test]
    fn anomaly_event_records_reason_and_cv_and_marks_the_span_failed() {
        let ((), spans) = capture_spans(|| {
            let _span = begin_acquire(1, "percentile");
            event_anomaly_detected("high coefficient of variation", 0.75);
        });

        let span = &spans[0];
        assert_eq!(span.status, Status::error("high coefficient of variation"));
        let events = &span.events.events;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].name, "anomaly.detected");
        assert_eq!(
            events[0].attributes,
            [
                KeyValue::new("baseline.anomaly.reason", "high coefficient of variation"),
                KeyValue::new("baseline.anomaly.cv", 0.75),
            ]
        );
    }

    #[test]
    fn baseline_metrics_accumulate_counts_and_keep_the_latest_tolerance() {
        let capture = MetricCapture::new();
        let meter = capture.meter(TRACER);
        record_baseline_gauges_on(&meter, 5, 500, 10.0, 90.0);
        record_baseline_gauges_on(&meter, 2, 40, 12.5, 80.0);

        let value =
            |name: &str| -> Vec<f64> { capture.points(name).iter().map(|p| p.value).collect() };
        assert_eq!(value("tumult.baseline.probes.total"), [7.0]);
        assert_eq!(value("tumult.baseline.samples.total"), [540.0]);
        assert_eq!(value("tumult.baseline.tolerance.lower"), [12.5]);
        assert_eq!(value("tumult.baseline.tolerance.upper"), [80.0]);
    }
}
