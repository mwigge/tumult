//! In-memory capture of the spans and metrics a piece of code emits.

use std::future::Future;
use std::sync::OnceLock;

use opentelemetry::metrics::{Meter, MeterProvider};
use opentelemetry::trace::{Span, TraceContextExt, TraceId, Tracer};
use opentelemetry::{global, Context, KeyValue, Value};
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
use opentelemetry_sdk::propagation::TraceContextPropagator;
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};

const CAPTURE_ROOT: &str = "tumult-test-utils.capture";

/// Installs the process-wide recording tracer provider and the W3C
/// propagator, once, and returns its exporter.
///
/// The global provider is never swapped back: tests in one binary run on
/// several threads, and a provider that changes under them is what this
/// helper exists to avoid.
fn exporter() -> &'static InMemorySpanExporter {
    static EXPORTER: OnceLock<InMemorySpanExporter> = OnceLock::new();
    EXPORTER.get_or_init(|| {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        global::set_tracer_provider(provider);
        global::set_text_map_propagator(TraceContextPropagator::new());
        exporter
    })
}

/// Makes the global tracer record spans and propagate W3C trace context.
///
/// For tests that need valid span contexts but assert on something else, for
/// example a `traceparent` header seen by a fake server. Tests that assert on
/// spans use [`capture_spans`].
pub fn install_recording_tracer() {
    let _ = exporter();
}

fn capture_root() -> (Context, TraceId) {
    let _ = exporter();
    let root =
        global::tracer("tumult-test-utils").start_with_context(CAPTURE_ROOT, &Context::new());
    let trace_id = root.span_context().trace_id();
    (Context::new().with_span(root), trace_id)
}

fn finished(root: &Context, trace_id: TraceId) -> Vec<SpanData> {
    root.span().end();
    exporter()
        .get_finished_spans()
        .expect("in-memory span exporter is readable")
        .into_iter()
        .filter(|span| span.span_context.trace_id() == trace_id && span.name != CAPTURE_ROOT)
        .collect()
}

/// Runs `f` and returns its result with the spans it finished, in end order.
///
/// `f` runs inside a fresh trace, and only spans of that trace are returned,
/// so tests running in parallel in the same process never see each other's
/// spans. A span is returned once it has ended; code under test that starts a
/// new root with an explicit empty parent context is not captured.
pub fn capture_spans<T>(f: impl FnOnce() -> T) -> (T, Vec<SpanData>) {
    let (root, trace_id) = capture_root();
    let out = {
        let _attached = root.clone().attach();
        f()
    };
    (out, finished(&root, trace_id))
}

/// Async form of [`capture_spans`].
///
/// The capture trace stays attached to the calling thread for as long as the
/// future runs, which matches the telemetry helpers under test: they attach a
/// span for the length of an async call. Await this directly in the test
/// function; the returned future is not `Send`. Tasks the future spawns are
/// captured only if the code under test hands them its context.
pub async fn capture_spans_async<F: Future>(future: F) -> (F::Output, Vec<SpanData>) {
    let (root, trace_id) = capture_root();
    let out = {
        let _attached = root.clone().attach();
        future.await
    };
    (out, finished(&root, trace_id))
}

/// The value of attribute `key` on `span`, if set.
#[must_use]
pub fn span_attribute<'a>(span: &'a SpanData, key: &str) -> Option<&'a Value> {
    attribute(&span.attributes, key)
}

/// The value of `key` in a list of attributes, if present.
#[must_use]
pub fn attribute<'a>(attributes: &'a [KeyValue], key: &str) -> Option<&'a Value> {
    attributes
        .iter()
        .find(|kv| kv.key.as_str() == key)
        .map(|kv| &kv.value)
}

/// One data point of a counter or gauge.
#[derive(Debug, Clone, PartialEq)]
pub struct MetricPoint {
    /// The point's value. Integer instruments are widened to `f64`.
    pub value: f64,
    /// The attributes the value was recorded with.
    pub attributes: Vec<KeyValue>,
}

/// A private meter provider whose recorded values can be read back.
///
/// Nothing is installed globally: the code under test records on a
/// [`Meter`] from [`MetricCapture::meter`].
pub struct MetricCapture {
    provider: SdkMeterProvider,
    exporter: InMemoryMetricExporter,
}

impl Default for MetricCapture {
    fn default() -> Self {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        Self { provider, exporter }
    }
}

impl MetricCapture {
    /// Creates an empty capture.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A meter that records into this capture.
    #[must_use]
    pub fn meter(&self, name: &'static str) -> Meter {
        self.provider.meter(name)
    }

    /// The current data points of the counter or gauge called `name`; empty
    /// when nothing was recorded under that name.
    ///
    /// # Panics
    ///
    /// Panics if the in-memory pipeline cannot be flushed or read, or if
    /// `name` is a histogram.
    #[must_use]
    pub fn points(&self, name: &str) -> Vec<MetricPoint> {
        self.provider
            .force_flush()
            .expect("in-memory metric pipeline flushes");
        let exports = self
            .exporter
            .get_finished_metrics()
            .expect("in-memory metric exporter is readable");
        // Sums are cumulative, so the newest export holds the current state.
        let points = exports
            .iter()
            .rev()
            .flat_map(|export| export.scope_metrics())
            .flat_map(|scope| scope.metrics())
            .find(|metric| metric.name() == name)
            .map(|metric| match metric.data() {
                AggregatedMetrics::F64(data) => points_of(data, |v| v),
                #[allow(clippy::cast_precision_loss)] // test values are small
                AggregatedMetrics::U64(data) => points_of(data, |v| v as f64),
                #[allow(clippy::cast_precision_loss)] // test values are small
                AggregatedMetrics::I64(data) => points_of(data, |v| v as f64),
            })
            .unwrap_or_default();
        points
    }
}

fn points_of<T: Copy>(data: &MetricData<T>, widen: impl Fn(T) -> f64) -> Vec<MetricPoint> {
    match data {
        MetricData::Gauge(gauge) => gauge
            .data_points()
            .map(|point| MetricPoint {
                value: widen(point.value()),
                attributes: point.attributes().cloned().collect(),
            })
            .collect(),
        MetricData::Sum(sum) => sum
            .data_points()
            .map(|point| MetricPoint {
                value: widen(point.value()),
                attributes: point.attributes().cloned().collect(),
            })
            .collect(),
        MetricData::Histogram(_) | MetricData::ExponentialHistogram(_) => {
            panic!("MetricCapture::points reads counters and gauges, not histograms")
        }
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::trace::{SpanKind, Status};

    use super::*;

    fn emit(name: &'static str) {
        let tracer = global::tracer("unit");
        let mut span = tracer
            .span_builder(name)
            .with_kind(SpanKind::Client)
            .with_attributes([KeyValue::new("k", "v")])
            .start(&tracer);
        span.add_event("happened", vec![KeyValue::new("n", 1_i64)]);
        span.set_status(Status::error("boom"));
    }

    #[test]
    fn capture_returns_the_spans_the_closure_finished() {
        let (out, spans) = capture_spans(|| {
            emit("one");
            emit("two");
            7
        });

        assert_eq!(out, 7);
        let names: Vec<&str> = spans.iter().map(|s| s.name.as_ref()).collect();
        assert_eq!(names, ["one", "two"]);
        assert_eq!(spans[0].span_kind, SpanKind::Client);
        assert_eq!(span_attribute(&spans[0], "k"), Some(&Value::from("v")));
        assert_eq!(span_attribute(&spans[0], "missing"), None);
        assert_eq!(spans[0].status, Status::error("boom"));
        let event = &spans[0].events.events[0];
        assert_eq!(event.name, "happened");
        assert_eq!(attribute(&event.attributes, "n"), Some(&Value::I64(1)));
    }

    #[test]
    fn capture_excludes_spans_from_outside_the_closure() {
        emit("before");
        let (inside, outside) = std::thread::scope(|scope| {
            let (started, wait_started) = std::sync::mpsc::channel();
            let (finish, wait_finish) = std::sync::mpsc::channel::<()>();
            // A second capture that overlaps the first one in time.
            let other = scope.spawn(move || {
                capture_spans(|| {
                    emit("other-test");
                    started.send(()).unwrap();
                    wait_finish.recv().unwrap();
                })
                .1
            });
            wait_started.recv().unwrap();
            let ((), inside) = capture_spans(|| emit("inside"));
            finish.send(()).unwrap();
            (inside, other.join().unwrap())
        });
        emit("after");

        let names = |spans: &[SpanData]| -> Vec<String> {
            spans.iter().map(|s| s.name.to_string()).collect()
        };
        assert_eq!(names(&inside), ["inside"]);
        assert_eq!(names(&outside), ["other-test"]);
    }

    #[test]
    fn captured_spans_are_children_of_one_trace() {
        let ((), spans) = capture_spans(|| {
            let tracer = global::tracer("unit");
            let parent = tracer.start("parent");
            let cx = Context::current_with_span(parent);
            let _attached = cx.attach();
            emit("child");
        });

        let child = spans.iter().find(|s| s.name == "child").unwrap();
        let parent = spans.iter().find(|s| s.name == "parent").unwrap();
        assert_eq!(child.parent_span_id, parent.span_context.span_id());
        assert_eq!(
            child.span_context.trace_id(),
            parent.span_context.trace_id()
        );
    }

    #[tokio::test]
    async fn async_capture_follows_the_future_across_awaits() {
        let (out, spans) = capture_spans_async(async {
            emit("before-await");
            tokio::task::yield_now().await;
            emit("after-await");
            "done"
        })
        .await;

        assert_eq!(out, "done");
        let names: Vec<&str> = spans.iter().map(|s| s.name.as_ref()).collect();
        assert_eq!(names, ["before-await", "after-await"]);
    }

    #[test]
    fn installed_tracer_propagates_w3c_trace_context() {
        install_recording_tracer();
        let ((), spans) = capture_spans(|| {
            let mut carrier = std::collections::HashMap::new();
            global::get_text_map_propagator(|propagator| {
                propagator.inject_context(&Context::current(), &mut carrier);
            });
            let trace_id = Context::current().span().span_context().trace_id();
            let traceparent = carrier.get("traceparent").expect("traceparent injected");
            assert!(
                traceparent.starts_with(&format!("00-{trace_id}-")),
                "{traceparent}"
            );
            emit("marker");
        });
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn metric_capture_reads_counters_and_gauges_with_attributes() {
        let capture = MetricCapture::new();
        let meter = capture.meter("unit");
        let counter = meter.u64_counter("hits").build();
        counter.add(2, &[KeyValue::new("ok", true)]);
        counter.add(3, &[KeyValue::new("ok", true)]);
        counter.add(1, &[KeyValue::new("ok", false)]);
        meter.f64_gauge("level").build().record(1.5, &[]);

        let mut hits = capture.points("hits");
        hits.sort_by(|a, b| a.value.total_cmp(&b.value));
        assert_eq!(
            hits,
            [
                MetricPoint {
                    value: 1.0,
                    attributes: vec![KeyValue::new("ok", false)],
                },
                MetricPoint {
                    value: 5.0,
                    attributes: vec![KeyValue::new("ok", true)],
                },
            ]
        );
        assert_eq!(
            capture.points("level"),
            [MetricPoint {
                value: 1.5,
                attributes: vec![],
            }]
        );
        assert_eq!(capture.points("never-recorded"), []);
    }

    #[test]
    fn metric_capture_reports_the_latest_cumulative_value() {
        let capture = MetricCapture::new();
        let counter = capture.meter("unit").u64_counter("hits").build();
        counter.add(1, &[]);
        assert_eq!(capture.points("hits")[0].value, 1.0);
        counter.add(4, &[]);
        assert_eq!(capture.points("hits")[0].value, 5.0);
    }
}
