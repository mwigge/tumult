//! Standard metrics for Tumult experiments.

use opentelemetry::metrics::{Counter, Gauge, Histogram, Meter};

/// Standard metrics emitted by the Tumult engine.
pub struct TumultMetrics {
    pub experiments_total: Counter<u64>,
    pub actions_total: Counter<u64>,
    pub probes_total: Counter<u64>,
    pub action_duration_seconds: Histogram<f64>,
    pub probe_duration_seconds: Histogram<f64>,
    pub hypothesis_deviations_total: Counter<u64>,
    pub plugin_errors_total: Counter<u64>,
    pub recovery_time_seconds: Gauge<f64>,
}

impl TumultMetrics {
    /// Creates a new set of standard Tumult metrics from the given `Meter`.
    #[must_use]
    pub fn new(meter: &Meter) -> Self {
        Self {
            experiments_total: meter
                .u64_counter("tumult_experiments_total")
                .with_description("Total experiments executed")
                .build(),
            actions_total: meter
                .u64_counter("tumult_actions_total")
                .with_description("Total actions executed")
                .build(),
            probes_total: meter
                .u64_counter("tumult_probes_total")
                .with_description("Total probes executed")
                .build(),
            action_duration_seconds: meter
                .f64_histogram("tumult_action_duration_seconds")
                .with_description("Action execution duration")
                .with_unit("s")
                .with_boundaries(vec![
                    0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0, 600.0,
                ])
                .build(),
            probe_duration_seconds: meter
                .f64_histogram("tumult_probe_duration_seconds")
                .with_description("Probe execution duration")
                .with_unit("s")
                .with_boundaries(vec![0.01, 0.05, 0.1, 0.5, 1.0, 2.0, 5.0, 10.0, 30.0, 60.0])
                .build(),
            hypothesis_deviations_total: meter
                .u64_counter("tumult_hypothesis_deviations_total")
                .with_description("Total steady-state hypothesis deviations")
                .build(),
            plugin_errors_total: meter
                .u64_counter("tumult_plugin_errors_total")
                .with_description("Total plugin execution errors")
                .build(),
            recovery_time_seconds: meter
                .f64_gauge("resilience.outcome.recovery_time_s")
                .with_description("Time in seconds for the system to recover after fault injection")
                .with_unit("s")
                .build(),
        }
    }
}
