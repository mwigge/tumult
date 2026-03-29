//! Tumult OTel — OpenTelemetry instrumentation for the Tumult platform.
//!
//! Provides always-on tracing, metrics, and logging via the
//! `tracing` + `tracing-opentelemetry` bridge with OTLP export.

pub mod attributes;
pub mod config;
pub mod metrics;
pub mod telemetry;

pub use config::TelemetryConfig;
pub use metrics::TumultMetrics;
pub use telemetry::TumultTelemetry;

#[cfg(test)]
mod tests {
    use super::*;

    // ── TelemetryConfig ────────────────────────────────────────

    #[test]
    fn config_defaults_are_sensible() {
        let config = TelemetryConfig::default();
        assert!(config.enabled);
        assert_eq!(config.service_name, "tumult");
        assert!(!config.console_export);
    }

    #[test]
    fn config_from_env_respects_disabled() {
        // Save and restore env vars
        let prev = std::env::var("TUMULT_OTEL_ENABLED").ok();
        std::env::set_var("TUMULT_OTEL_ENABLED", "false");
        let config = TelemetryConfig::from_env();
        assert!(!config.enabled);
        // Restore
        match prev {
            Some(v) => std::env::set_var("TUMULT_OTEL_ENABLED", v),
            None => std::env::remove_var("TUMULT_OTEL_ENABLED"),
        }
    }

    #[test]
    fn config_from_env_defaults_to_enabled() {
        let prev = std::env::var("TUMULT_OTEL_ENABLED").ok();
        std::env::remove_var("TUMULT_OTEL_ENABLED");
        let config = TelemetryConfig::from_env();
        assert!(config.enabled);
        if let Some(v) = prev {
            std::env::set_var("TUMULT_OTEL_ENABLED", v);
        }
    }

    // ── Attribute constants ────────────────────────────────────

    #[test]
    fn attribute_constants_use_resilience_namespace() {
        assert!(attributes::EXPERIMENT_ID.starts_with("resilience."));
        assert!(attributes::EXPERIMENT_NAME.starts_with("resilience."));
        assert!(attributes::ACTION_NAME.starts_with("resilience."));
        assert!(attributes::PROBE_NAME.starts_with("resilience."));
        assert!(attributes::TARGET_SYSTEM.starts_with("resilience."));
        assert!(attributes::OUTCOME.starts_with("resilience."));
        assert!(attributes::FAULT_TYPE.starts_with("resilience."));
    }

    // ── TumultMetrics ──────────────────────────────────────────

    #[test]
    fn metrics_can_be_created() {
        // This test verifies the metrics struct can be instantiated
        // without panicking. Actual metric recording is tested via
        // integration tests with an in-memory exporter.
        let meter = opentelemetry::global::meter("test");
        let metrics = TumultMetrics::new(&meter);
        // Verify the struct was created (no panic)
        drop(metrics);
    }

    // ── TumultTelemetry ────────────────────────────────────────

    #[test]
    fn telemetry_disabled_config_does_not_initialize_providers() {
        let config = TelemetryConfig {
            enabled: false,
            ..TelemetryConfig::default()
        };
        // Should not panic even when disabled
        let telemetry = TumultTelemetry::new(config);
        assert!(!telemetry.is_enabled());
    }
}
