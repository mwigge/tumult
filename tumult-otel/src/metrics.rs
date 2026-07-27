//! Standard metrics for Tumult experiments.
//!
//! Instrument names use the dot-separated `tumult.*` convention that the
//! bundled `SigNoz` dashboards (`docker/signoz/dashboards/`) query. Instruments
//! are built from the global meter, so they are no-ops until
//! [`crate::telemetry::TumultTelemetry`] installs a `MeterProvider`.

use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};

/// Standard metrics emitted by the Tumult engine.
pub struct TumultMetrics {
    pub(crate) experiments_total: Counter<u64>,
    pub(crate) actions_total: Counter<u64>,
    pub(crate) probes_total: Counter<u64>,
    pub(crate) action_duration_seconds: Histogram<f64>,
    pub(crate) probe_duration_seconds: Histogram<f64>,
    pub(crate) experiment_duration_seconds: Histogram<f64>,
    pub(crate) rollbacks_total: Counter<u64>,
    pub(crate) hypothesis_deviations_total: Counter<u64>,
    pub(crate) plugin_errors_total: Counter<u64>,
    // Intentionally not yet wired to a `record_recovery_time` function;
    // the gauge is emitted when the runner computes MTTR. Suppressing the
    // lint here until the recording site is added in a follow-up commit.
    #[allow(dead_code)]
    pub(crate) recovery_time_seconds: Gauge<f64>,
}

impl TumultMetrics {
    /// Creates a new set of standard Tumult metrics from the given `Meter`.
    #[must_use]
    pub fn new(meter: &Meter) -> Self {
        Self {
            experiments_total: meter
                .u64_counter("tumult.experiments.total")
                .with_description("Total experiments executed")
                .build(),
            actions_total: meter
                .u64_counter("tumult.actions.total")
                .with_description("Total actions executed")
                .build(),
            probes_total: meter
                .u64_counter("tumult.probes.total")
                .with_description("Total probes executed")
                .build(),
            action_duration_seconds: meter
                .f64_histogram("tumult.action.duration")
                .with_description("Action execution duration")
                .with_unit("s")
                .with_boundaries(vec![
                    0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0,
                ])
                .build(),
            probe_duration_seconds: meter
                .f64_histogram("tumult.probe.duration")
                .with_description("Probe execution duration")
                .with_unit("s")
                .with_boundaries(vec![0.01, 0.05, 0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0])
                .build(),
            experiment_duration_seconds: meter
                .f64_histogram("tumult.experiment.duration")
                .with_description("End-to-end experiment lifecycle duration")
                .with_unit("s")
                .with_boundaries(vec![
                    1.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1800.0, 3600.0,
                ])
                .build(),
            rollbacks_total: meter
                .u64_counter("tumult.rollbacks.total")
                .with_description("Total rollback activities executed")
                .build(),
            hypothesis_deviations_total: meter
                .u64_counter("tumult.hypothesis.deviations.total")
                .with_description("Total steady-state hypothesis deviations")
                .build(),
            plugin_errors_total: meter
                .u64_counter("tumult.plugin.errors.total")
                .with_description("Total plugin execution errors")
                .build(),
            recovery_time_seconds: meter
                .f64_gauge("tumult.recovery.time.seconds")
                .with_description("Time in seconds for the system to recover after fault injection")
                .with_unit("s")
                .build(),
        }
    }

    /// The process-wide metric set, built lazily from the global meter.
    ///
    /// Safe to call anywhere: before a `MeterProvider` is installed the
    /// instruments record into the noop meter. Note the instruments are bound
    /// to the meter that was global at first call — tests that install their
    /// own provider should construct `TumultMetrics::new` directly instead.
    pub fn global() -> &'static Self {
        static METRICS: std::sync::OnceLock<TumultMetrics> = std::sync::OnceLock::new();
        METRICS.get_or_init(|| Self::new(&opentelemetry::global::meter("tumult")))
    }
}
