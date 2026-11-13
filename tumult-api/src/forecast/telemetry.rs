//! OpenTelemetry instrumentation for the forecasting endpoints.
//!
//! ## Spans
//! - `forecast.produce` — one forecast, whoever made it. `outcome` is
//!   `sidecar`, `built_in`, `fallback` or `error`.
//! - `forecast.sidecar_call` — one `POST /forecast` to the sidecar, a child
//!   of the above. `outcome` is `ok` or the coarse failure code
//!   (`unreachable`, `timeout`, `bad_response`, `rejected`).
//!
//! ## Counters
//! - `tumult.forecast.fallbacks` — the built-in provider stepped in for a
//!   configured sidecar; `reason` is the coarse code, `circuit_open`
//!   included.
//! - `tumult.forecast.breaker.transitions` — the circuit changed state;
//!   `state` is the state entered (`open`, `half_open`, `closed`).
//! - `tumult.forecast.cache.lookups` — a metric forecast was looked up in
//!   the lake; `result` is `hit` or `miss`.
//!
//! The spans are held as a [`Context`] rather than attached to the thread,
//! so they can live across `await` points.

use opentelemetry::metrics::Meter;
use opentelemetry::trace::{TraceContextExt, Tracer};
use opentelemetry::{global, Context, KeyValue};

const SCOPE: &str = "tumult-api";

pub(crate) const PRODUCE: &str = "forecast.produce";
pub(crate) const SIDECAR_CALL: &str = "forecast.sidecar_call";
const FALLBACKS: &str = "tumult.forecast.fallbacks";
const BREAKER_TRANSITIONS: &str = "tumult.forecast.breaker.transitions";
const CACHE_LOOKUPS: &str = "tumult.forecast.cache.lookups";

/// A started span; [`Timed::finish`] stamps its outcome and ends it.
pub(crate) struct Timed {
    cx: Context,
}

impl Timed {
    fn start_in<T>(tracer: &T, name: &'static str, parent: &Context) -> Self
    where
        T: Tracer,
        T::Span: Send + Sync + 'static,
    {
        let span = tracer.start_with_context(name, parent);
        Self {
            cx: parent.with_span(span),
        }
    }

    /// A span under whatever is current on this thread.
    pub(crate) fn start(name: &'static str) -> Self {
        Self::start_in(&global::tracer(SCOPE), name, &Context::current())
    }

    /// A span under this one.
    pub(crate) fn child(&self, name: &'static str) -> Self {
        Self::start_in(&global::tracer(SCOPE), name, &self.cx)
    }

    /// Record how it went and end the span.
    pub(crate) fn finish(self, outcome: &'static str) {
        let span = self.cx.span();
        span.set_attribute(KeyValue::new("outcome", outcome));
        span.end();
    }
}

fn count(meter: &Meter, name: &'static str, key: &'static str, value: &'static str) {
    meter
        .u64_counter(name)
        .build()
        .add(1, &[KeyValue::new(key, value)]);
}

/// The built-in provider answered for a configured sidecar.
pub(crate) fn fallback(reason: &'static str) {
    count(&global::meter(SCOPE), FALLBACKS, "reason", reason);
}

/// The circuit entered `state`.
pub(crate) fn breaker_transition(state: &'static str) {
    count(&global::meter(SCOPE), BREAKER_TRANSITIONS, "state", state);
}

/// A cache lookup found a row (`hit`) or did not.
pub(crate) fn cache_lookup(hit: bool) {
    let result = if hit { "hit" } else { "miss" };
    count(&global::meter(SCOPE), CACHE_LOOKUPS, "result", result);
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::metrics::MeterProvider;
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

    #[test]
    fn spans_carry_their_outcome_and_nest() {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let tracer = provider.tracer("test");
        let produce = Timed::start_in(&tracer, PRODUCE, &Context::new());
        let call = Timed::start_in(&tracer, SIDECAR_CALL, &produce.cx);
        call.finish("timeout");
        produce.finish("fallback");

        let spans = exporter.get_finished_spans().unwrap();
        let outcome = |name: &str| -> String {
            let span = spans.iter().find(|s| s.name == name).unwrap();
            let attr = span
                .attributes
                .iter()
                .find(|kv| kv.key.as_str() == "outcome");
            attr.unwrap().value.as_str().to_string()
        };
        assert_eq!(outcome("forecast.produce"), "fallback");
        assert_eq!(outcome("forecast.sidecar_call"), "timeout");
        let parent = spans.iter().find(|s| s.name == "forecast.produce").unwrap();
        let child = spans
            .iter()
            .find(|s| s.name == "forecast.sidecar_call")
            .unwrap();
        assert_eq!(child.parent_span_id, parent.span_context.span_id());
        assert_eq!(
            child.span_context.trace_id(),
            parent.span_context.trace_id()
        );
    }

    /// `(metric, attribute value) -> count` for every sum the exporter saw.
    fn sums(exporter: &InMemoryMetricExporter) -> Vec<(String, String, u64)> {
        let mut out = Vec::new();
        for resource in exporter.get_finished_metrics().unwrap() {
            for metric in resource.scope_metrics().flat_map(|s| s.metrics()) {
                let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() else {
                    continue;
                };
                for point in sum.data_points() {
                    let label = point
                        .attributes()
                        .map(|kv| kv.value.as_str().to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                    out.push((metric.name().to_string(), label, point.value()));
                }
            }
        }
        out
    }

    #[test]
    fn counters_are_labelled_by_what_happened() {
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_reader(PeriodicReader::builder(exporter.clone()).build())
            .build();
        let meter = provider.meter("test");
        count(&meter, FALLBACKS, "reason", "circuit_open");
        count(&meter, FALLBACKS, "reason", "circuit_open");
        count(&meter, FALLBACKS, "reason", "timeout");
        count(&meter, BREAKER_TRANSITIONS, "state", "half_open");
        count(&meter, CACHE_LOOKUPS, "result", "hit");
        provider.force_flush().unwrap();

        let seen = sums(&exporter);
        let value = |name: &str, label: &str| -> Option<u64> {
            seen.iter()
                .find(|(n, l, _)| n == name && l == label)
                .map(|(_, _, v)| *v)
        };
        assert_eq!(value("tumult.forecast.fallbacks", "circuit_open"), Some(2));
        assert_eq!(value("tumult.forecast.fallbacks", "timeout"), Some(1));
        assert_eq!(
            value("tumult.forecast.breaker.transitions", "half_open"),
            Some(1)
        );
        assert_eq!(value("tumult.forecast.cache.lookups", "hit"), Some(1));
    }

    #[test]
    fn the_global_entry_points_are_safe_without_a_provider() {
        let produce = Timed::start(PRODUCE);
        produce.child(SIDECAR_CALL).finish("ok");
        produce.finish("sidecar");
        fallback("unreachable");
        breaker_transition("open");
        cache_lookup(true);
        cache_lookup(false);
    }
}
