//! Instrumentation wrappers for chaos actions and probes.
//!
//! Every action and probe execution is wrapped in an `OTel` span
//! with `resilience.*` attributes. This is the always-on observability layer.

use std::time::Instant;

use opentelemetry::trace::{SpanKind, TraceContextExt, Tracer};
use opentelemetry::{global, KeyValue};

use crate::attributes;
use crate::metrics::TumultMetrics;

/// RAII guard that holds an OpenTelemetry context attachment.
///
/// Keeps the span active for the lifetime of the guard. Drop the guard
/// to detach the context and end the span's "active" window.
pub struct SpanGuard {
    /// Dropping this detaches the associated `Context` from the current thread.
    // Held solely for its `Drop` side-effect; never read directly.
    #[allow(dead_code)]
    guard: opentelemetry::ContextGuard,
}

impl SpanGuard {
    /// Creates a new guard from an `OTel` context guard.
    #[must_use]
    pub fn new(guard: opentelemetry::ContextGuard) -> Self {
        Self { guard }
    }
}

/// Start a `Client`-kind span on the named global tracer, attach it to the
/// current context, and return a [`SpanGuard`] that ends the span's active
/// window on drop.
///
/// Shared by the plugin crates (`tumult-net`, `tumult-kubernetes`,
/// `tumult-ssh`) for their `<domain>.*` client spans. Both names are
/// `&'static str` so the `Into<Cow<'static, str>>` conversions are zero-cost.
#[must_use]
pub fn client_span(
    tracer_name: &'static str,
    span_name: &'static str,
    attrs: Vec<KeyValue>,
) -> SpanGuard {
    let tracer = global::tracer(tracer_name);
    let span = tracer
        .span_builder(span_name)
        .with_kind(SpanKind::Client)
        .with_attributes(attrs)
        .start(&tracer);
    let cx = opentelemetry::Context::current_with_span(span);
    SpanGuard::new(cx.attach())
}

/// Result of an instrumented operation.
#[derive(Debug, Clone)]
pub struct InstrumentedResult {
    pub duration_ms: u64,
    pub success: bool,
    pub output: Option<String>,
    pub error: Option<String>,
}

/// Record the execution of an action in `OTel` spans and metrics.
pub fn record_action(
    metrics: &TumultMetrics,
    plugin_name: &str,
    action_name: &str,
    start: Instant,
    success: bool,
) {
    let duration = start.elapsed();
    let duration_s = duration.as_secs_f64();
    let outcome = if success { "success" } else { "failure" };

    let attrs = &[
        KeyValue::new(attributes::PLUGIN_NAME, plugin_name.to_string()),
        KeyValue::new(attributes::ACTION_NAME, action_name.to_string()),
        KeyValue::new(attributes::OUTCOME, outcome),
    ];

    metrics.actions_total.add(1, attrs);
    metrics.action_duration_seconds.record(duration_s, attrs);

    if !success {
        metrics.plugin_errors_total.add(1, attrs);
    }
}

/// Record the execution of a probe in `OTel` spans and metrics.
pub fn record_probe(
    metrics: &TumultMetrics,
    plugin_name: &str,
    probe_name: &str,
    start: Instant,
    success: bool,
) {
    let duration = start.elapsed();
    let duration_s = duration.as_secs_f64();
    let outcome = if success { "success" } else { "failure" };

    let attrs = &[
        KeyValue::new(attributes::PLUGIN_NAME, plugin_name.to_string()),
        KeyValue::new(attributes::PROBE_NAME, probe_name.to_string()),
        KeyValue::new(attributes::OUTCOME, outcome),
    ];

    metrics.probes_total.add(1, attrs);
    metrics.probe_duration_seconds.record(duration_s, attrs);

    if !success {
        metrics.plugin_errors_total.add(1, attrs);
    }
}

/// Record a hypothesis deviation, tagged by experiment name.
///
/// The `experiment_name` attribute allows downstream dashboards and
/// alerts to break down deviation counts per experiment.
pub fn record_deviation(metrics: &TumultMetrics, experiment_name: &str) {
    metrics.hypothesis_deviations_total.add(
        1,
        &[KeyValue::new(
            attributes::EXPERIMENT_NAME,
            experiment_name.to_string(),
        )],
    );
}

/// Record experiment completion.
pub fn record_experiment(metrics: &TumultMetrics, success: bool) {
    let outcome = if success { "success" } else { "failure" };
    metrics
        .experiments_total
        .add(1, &[KeyValue::new(attributes::OUTCOME, outcome)]);
}

/// Record the end-to-end duration of a completed experiment lifecycle.
pub fn record_experiment_duration(metrics: &TumultMetrics, duration_s: f64, success: bool) {
    let outcome = if success { "success" } else { "failure" };
    metrics
        .experiment_duration_seconds
        .record(duration_s, &[KeyValue::new(attributes::OUTCOME, outcome)]);
}

/// Record the execution of a rollback activity.
pub fn record_rollback(metrics: &TumultMetrics, rollback_name: &str, success: bool) {
    let outcome = if success { "success" } else { "failure" };
    metrics.rollbacks_total.add(
        1,
        &[
            KeyValue::new(attributes::ACTION_NAME, rollback_name.to_string()),
            KeyValue::new(attributes::OUTCOME, outcome),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Build an isolated SDK `MeterProvider` backed by an `InMemoryMetricExporter`
    /// so that metric data points can be inspected in tests without touching the
    /// global provider (TEST-01).
    #[cfg(test)]
    fn sdk_harness() -> (
        opentelemetry::metrics::Meter,
        opentelemetry_sdk::metrics::InMemoryMetricExporter,
        opentelemetry_sdk::metrics::SdkMeterProvider,
    ) {
        use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};
        let exporter = InMemoryMetricExporter::default();
        let provider = SdkMeterProvider::builder()
            .with_periodic_exporter(exporter.clone())
            .build();
        let meter = opentelemetry::metrics::MeterProvider::meter(&provider, "test");
        (meter, exporter, provider)
    }

    /// Collect all exported metric names after a `force_flush`.
    fn flush_and_names(
        provider: &opentelemetry_sdk::metrics::SdkMeterProvider,
        exporter: &opentelemetry_sdk::metrics::InMemoryMetricExporter,
    ) -> Vec<String> {
        provider.force_flush().unwrap();
        exporter
            .get_finished_metrics()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|rm| {
                rm.scope_metrics()
                    .flat_map(|sm| sm.metrics().map(|m| m.name().to_owned()))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    // ── Behavioral assertions (TEST-01) ──────────────────────────────────────

    #[test]
    fn record_action_success_increments_actions_total() {
        let (meter, exporter, provider) = sdk_harness();
        let metrics = TumultMetrics::new(&meter);
        let start = Instant::now();
        record_action(&metrics, "tumult-db", "kill-connections", start, true);
        let names = flush_and_names(&provider, &exporter);
        assert!(
            names.iter().any(|n| n == "tumult.actions.total"),
            "expected tumult.actions.total in {names:?}"
        );
        // On success, plugin_errors_total must NOT be reported (no data points recorded).
        assert!(
            !names.iter().any(|n| n == "tumult.plugin.errors.total"),
            "plugin_errors_total must not be emitted on success; found in {names:?}"
        );
    }

    #[test]
    fn record_action_failure_increments_actions_total_and_plugin_errors_total() {
        let (meter, exporter, provider) = sdk_harness();
        let metrics = TumultMetrics::new(&meter);
        let start = Instant::now();
        record_action(&metrics, "tumult-db", "kill-connections", start, false);
        let names = flush_and_names(&provider, &exporter);
        assert!(
            names.iter().any(|n| n == "tumult.actions.total"),
            "expected tumult.actions.total in {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "tumult.plugin.errors.total"),
            "expected tumult.plugin.errors.total on failure in {names:?}"
        );
    }

    #[test]
    fn record_action_records_duration_histogram() {
        let (meter, exporter, provider) = sdk_harness();
        let metrics = TumultMetrics::new(&meter);
        let start = Instant::now();
        record_action(&metrics, "tumult-db", "kill-connections", start, true);
        let names = flush_and_names(&provider, &exporter);
        assert!(
            names.iter().any(|n| n == "tumult.action.duration"),
            "expected tumult.action.duration histogram in {names:?}"
        );
    }

    #[test]
    fn record_probe_success_does_not_increment_plugin_errors_total() {
        let (meter, exporter, provider) = sdk_harness();
        let metrics = TumultMetrics::new(&meter);
        let start = Instant::now();
        record_probe(&metrics, "tumult-http", "health-check", start, true);
        let names = flush_and_names(&provider, &exporter);
        assert!(
            names.iter().any(|n| n == "tumult.probes.total"),
            "expected tumult.probes.total in {names:?}"
        );
        assert!(
            !names.iter().any(|n| n == "tumult.plugin.errors.total"),
            "plugin_errors_total must not be emitted on probe success"
        );
    }

    #[test]
    fn record_probe_failure_increments_plugin_errors_total() {
        let (meter, exporter, provider) = sdk_harness();
        let metrics = TumultMetrics::new(&meter);
        let start = Instant::now();
        record_probe(&metrics, "tumult-http", "health-check", start, false);
        let names = flush_and_names(&provider, &exporter);
        assert!(
            names.iter().any(|n| n == "tumult.plugin.errors.total"),
            "expected tumult.plugin.errors.total on probe failure in {names:?}"
        );
    }

    #[test]
    fn record_deviation_increments_hypothesis_deviations_total() {
        let (meter, exporter, provider) = sdk_harness();
        let metrics = TumultMetrics::new(&meter);
        record_deviation(&metrics, "db-connection-pool-exhaustion");
        let names = flush_and_names(&provider, &exporter);
        assert!(
            names
                .iter()
                .any(|n| n == "tumult.hypothesis.deviations.total"),
            "expected tumult.hypothesis.deviations.total in {names:?}"
        );
    }

    #[test]
    fn record_experiment_increments_experiments_total() {
        let (meter, exporter, provider) = sdk_harness();
        let metrics = TumultMetrics::new(&meter);
        record_experiment(&metrics, true);
        record_experiment(&metrics, false);
        let names = flush_and_names(&provider, &exporter);
        assert!(
            names.iter().any(|n| n == "tumult.experiments.total"),
            "expected tumult.experiments.total in {names:?}"
        );
    }

    // ── Attribute key correctness ─────────────────────────────────────────────

    /// Regression: `record_deviation` must tag with the canonical
    /// `resilience.experiment.name` attribute, not the legacy `.title` key.
    #[test]
    fn record_deviation_uses_canonical_experiment_name_attribute() {
        // The attribute key constant must match the canonical value.
        assert_eq!(attributes::EXPERIMENT_NAME, "resilience.experiment.name");
    }

    // ── InstrumentedResult ────────────────────────────────────────────────────

    #[test]
    fn instrumented_result_captures_all_fields() {
        let result = InstrumentedResult {
            duration_ms: 342,
            success: true,
            output: Some("pod deleted".into()),
            error: None,
        };
        assert!(result.success);
        assert_eq!(result.duration_ms, 342);
        assert_eq!(result.output.unwrap(), "pod deleted");
        assert!(result.error.is_none());
    }
}
