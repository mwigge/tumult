//! In-memory capture of the spans and metrics a piece of code emits.

use std::future::Future;
use std::sync::OnceLock;

use opentelemetry::metrics::{Meter, MeterProvider};
use opentelemetry::trace::{Span, TraceContextExt, TraceId, Tracer};
use opentelemetry::{global, Context, KeyValue, Value};
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ScopeMetrics};
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
///
/// Limit: the provider is global to the test binary. A test in the same
/// binary that calls `global::set_tracer_provider` itself replaces it, and
/// from then on every capture in that binary returns no spans. Such a test
/// belongs in a binary of its own (a separate file under `tests/`), or uses
/// this helper instead.
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
/// The provider is installed once per test binary and never replaced. A test
/// in the same binary that calls `global::set_tracer_provider` itself breaks
/// every span capture in that binary; keep such a test in a binary of its own.
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
///
/// Nothing is captured in a test binary where another test installs its own
/// global tracer provider; see [`install_recording_tracer`].
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

/// One data point of a histogram: everything recorded with one attribute set.
#[derive(Debug, Clone, PartialEq)]
pub struct HistogramPoint {
    /// How many values were recorded.
    pub count: u64,
    /// Their sum. Integer instruments are widened to `f64`.
    pub sum: f64,
    /// The attributes the values were recorded with.
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

    /// The capture that is the meter provider of the whole process,
    /// installed on first use and never replaced.
    ///
    /// For code that records on the global meter. The values add up over
    /// the whole test binary, so a test reads what its own code added, and
    /// the tests that do so run one at a time.
    #[must_use]
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<MetricCapture> = OnceLock::new();
        GLOBAL.get_or_init(|| {
            let capture = Self::new();
            global::set_meter_provider(capture.provider.clone());
            capture
        })
    }

    /// The sum of the points of the counter or gauge called `name` that
    /// carry every attribute of `with`, each a key and its text.
    ///
    /// # Panics
    ///
    /// As [`MetricCapture::points`].
    #[must_use]
    pub fn total(&self, name: &str, with: &[(&str, &str)]) -> f64 {
        self.points(name)
            .iter()
            .filter(|point| {
                with.iter().all(|(key, text)| {
                    attribute(&point.attributes, key).is_some_and(|found| found.as_str() == *text)
                })
            })
            .map(|point| point.value)
            .sum()
    }

    /// The current data points of the counter or gauge called `name`, from
    /// every meter that has an instrument of that name; empty when nothing
    /// was recorded under it.
    ///
    /// # Panics
    ///
    /// Panics if the in-memory pipeline cannot be flushed or read, or if
    /// `name` is a histogram: those are read with
    /// [`MetricCapture::histogram_points`].
    #[must_use]
    pub fn points(&self, name: &str) -> Vec<MetricPoint> {
        self.series(name)
            .into_iter()
            .flat_map(|series| match series {
                Series::Values(points) => points,
                Series::Histogram(_) => {
                    panic!("`{name}` is a histogram: read it with MetricCapture::histogram_points")
                }
            })
            .collect()
    }

    /// The current data points of the histogram called `name`, one per
    /// attribute set, from every meter that has an instrument of that name;
    /// empty when nothing was recorded under it.
    ///
    /// # Panics
    ///
    /// Panics if the in-memory pipeline cannot be flushed or read, or if
    /// `name` is a counter or gauge: those are read with
    /// [`MetricCapture::points`].
    #[must_use]
    pub fn histogram_points(&self, name: &str) -> Vec<HistogramPoint> {
        self.series(name)
            .into_iter()
            .flat_map(|series| match series {
                Series::Histogram(points) => points,
                Series::Values(_) => {
                    panic!("`{name}` is not a histogram: read it with MetricCapture::points")
                }
            })
            .collect()
    }

    /// Every metric called `name` in the newest export that has one.
    fn series(&self, name: &str) -> Vec<Series> {
        self.provider
            .force_flush()
            .expect("in-memory metric pipeline flushes");
        let exports = self
            .exporter
            .get_finished_metrics()
            .expect("in-memory metric exporter is readable");
        // Aggregation is cumulative, so the newest export holds the current
        // state. Two meters may each have an instrument of the same name.
        exports
            .iter()
            .rev()
            .map(|export| {
                export
                    .scope_metrics()
                    .flat_map(ScopeMetrics::metrics)
                    .filter(|metric| metric.name() == name)
                    .map(|metric| match metric.data() {
                        AggregatedMetrics::F64(data) => series_of(data, |v| v),
                        #[allow(clippy::cast_precision_loss)] // test values are small
                        AggregatedMetrics::U64(data) => series_of(data, |v| v as f64),
                        #[allow(clippy::cast_precision_loss)] // test values are small
                        AggregatedMetrics::I64(data) => series_of(data, |v| v as f64),
                    })
                    .collect::<Vec<_>>()
            })
            .find(|found| !found.is_empty())
            .unwrap_or_default()
    }
}

/// The data points of one metric, by instrument kind.
enum Series {
    Values(Vec<MetricPoint>),
    Histogram(Vec<HistogramPoint>),
}

fn series_of<T: Copy>(data: &MetricData<T>, widen: impl Fn(T) -> f64) -> Series {
    match data {
        MetricData::Gauge(gauge) => Series::Values(
            gauge
                .data_points()
                .map(|point| MetricPoint {
                    value: widen(point.value()),
                    attributes: point.attributes().cloned().collect(),
                })
                .collect(),
        ),
        MetricData::Sum(sum) => Series::Values(
            sum.data_points()
                .map(|point| MetricPoint {
                    value: widen(point.value()),
                    attributes: point.attributes().cloned().collect(),
                })
                .collect(),
        ),
        MetricData::Histogram(histogram) => Series::Histogram(
            histogram
                .data_points()
                .map(|point| HistogramPoint {
                    count: point.count(),
                    sum: widen(point.sum()),
                    attributes: point.attributes().cloned().collect(),
                })
                .collect(),
        ),
        MetricData::ExponentialHistogram(histogram) => Series::Histogram(
            histogram
                .data_points()
                .map(|point| HistogramPoint {
                    count: point.count() as u64,
                    sum: widen(point.sum()),
                    attributes: point.attributes().cloned().collect(),
                })
                .collect(),
        ),
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
    fn the_global_capture_reads_what_is_recorded_on_the_global_meter() {
        let capture = MetricCapture::global();
        let counter = global::meter("unit")
            .u64_counter("unit.global.total")
            .build();
        counter.add(2, &[KeyValue::new("kind", "a")]);
        counter.add(
            3,
            &[KeyValue::new("kind", "b"), KeyValue::new("side", "left")],
        );
        counter.add(
            4,
            &[KeyValue::new("kind", "b"), KeyValue::new("side", "right")],
        );

        assert_eq!(
            (
                capture.total("unit.global.total", &[]),
                capture.total("unit.global.total", &[("kind", "b")]),
                capture.total("unit.global.total", &[("kind", "b"), ("side", "left")]),
                capture.total("unit.global.total", &[("kind", "c")]),
                capture.total("unit.global.total", &[("side", "a")]),
                capture.total("unit.absent.total", &[]),
            ),
            (9.0, 7.0, 3.0, 0.0, 0.0, 0.0)
        );
        assert!(std::ptr::eq(capture, MetricCapture::global()));
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
        assert!((capture.points("hits")[0].value - 1.0).abs() < f64::EPSILON);
        counter.add(4, &[]);
        assert!((capture.points("hits")[0].value - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn metric_capture_returns_the_points_of_every_meter_that_uses_the_name() {
        let capture = MetricCapture::new();
        capture
            .meter("first")
            .u64_counter("hits")
            .build()
            .add(2, &[]);
        capture
            .meter("second")
            .u64_counter("hits")
            .build()
            .add(5, &[]);

        let mut values: Vec<f64> = capture.points("hits").iter().map(|p| p.value).collect();
        values.sort_by(f64::total_cmp);

        assert_eq!(values, [2.0, 5.0]);
    }

    #[test]
    fn metric_capture_reads_histograms_as_count_and_sum_per_attribute_set() {
        let capture = MetricCapture::new();
        let histogram = capture.meter("unit").f64_histogram("latency").build();
        histogram.record(1.5, &[KeyValue::new("route", "a")]);
        histogram.record(2.5, &[KeyValue::new("route", "a")]);
        histogram.record(10.0, &[KeyValue::new("route", "b")]);

        let mut points = capture.histogram_points("latency");
        points.sort_by(|a, b| a.sum.total_cmp(&b.sum));

        assert_eq!(
            points,
            [
                HistogramPoint {
                    count: 2,
                    sum: 4.0,
                    attributes: vec![KeyValue::new("route", "a")],
                },
                HistogramPoint {
                    count: 1,
                    sum: 10.0,
                    attributes: vec![KeyValue::new("route", "b")],
                },
            ]
        );
        assert_eq!(capture.histogram_points("never-recorded"), []);
    }

    #[test]
    #[should_panic(
        expected = "`latency` is a histogram: read it with MetricCapture::histogram_points"
    )]
    fn reading_a_histogram_as_points_says_which_method_to_use() {
        let capture = MetricCapture::new();
        capture
            .meter("unit")
            .u64_histogram("latency")
            .build()
            .record(3, &[]);

        let _ = capture.points("latency");
    }

    #[test]
    #[should_panic(expected = "`hits` is not a histogram: read it with MetricCapture::points")]
    fn reading_a_counter_as_a_histogram_says_which_method_to_use() {
        let capture = MetricCapture::new();
        capture
            .meter("unit")
            .u64_counter("hits")
            .build()
            .add(1, &[]);

        let _ = capture.histogram_points("hits");
    }
}
