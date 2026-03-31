//! Tumult `OTel` — OpenTelemetry instrumentation for the Tumult platform.
//!
//! Provides always-on tracing, metrics, and logging via the
//! `tracing` + `tracing-opentelemetry` bridge with OTLP export.

pub mod attributes;
pub mod config;
pub mod instrument;
pub mod metrics;
pub mod telemetry;

pub use config::TelemetryConfig;
pub use instrument::SpanGuard;
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

    // Env-var tests use a shared mutex to avoid race conditions when
    // multiple tests manipulate the same environment variable.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn config_from_env_respects_disabled() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let prev = std::env::var("TUMULT_OTEL_ENABLED").ok();
        std::env::set_var("TUMULT_OTEL_ENABLED", "false");
        let config = TelemetryConfig::from_env();
        assert!(!config.enabled);
        match prev {
            Some(v) => std::env::set_var("TUMULT_OTEL_ENABLED", v),
            None => std::env::remove_var("TUMULT_OTEL_ENABLED"),
        }
    }

    #[test]
    fn config_from_env_defaults_to_enabled() {
        let _guard = ENV_MUTEX.lock().unwrap();
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

    // ── Attribute coverage ──────────────────────────────────────

    #[test]
    fn all_attributes_are_dot_separated() {
        let attrs = [
            attributes::EXPERIMENT_ID,
            attributes::EXPERIMENT_NAME,
            attributes::EXPERIMENT_RUN_NUMBER,
            attributes::TARGET_SYSTEM,
            attributes::TARGET_TECHNOLOGY,
            attributes::TARGET_COMPONENT,
            attributes::TARGET_ENVIRONMENT,
            attributes::FAULT_TYPE,
            attributes::FAULT_SUBTYPE,
            attributes::FAULT_SEVERITY,
            attributes::FAULT_BLAST_RADIUS,
            attributes::ACTION_NAME,
            attributes::PROBE_NAME,
            attributes::PLUGIN_NAME,
            attributes::OUTCOME,
            attributes::HYPOTHESIS_MET,
            attributes::RECOVERY_TIME_S,
            attributes::EXECUTION_TARGET,
            attributes::DURATION_MS,
        ];
        for attr in attrs {
            assert!(
                attr.contains('.'),
                "attribute '{attr}' must be dot-separated"
            );
            assert!(
                attr.starts_with("resilience."),
                "attribute '{attr}' must start with 'resilience.'"
            );
        }
    }

    // ── Instrumentation integration ────────────────────────────

    #[test]
    fn record_action_and_probe_sequence_does_not_panic() {
        let meter = opentelemetry::global::meter("integration-test");
        let metrics = TumultMetrics::new(&meter);

        // Simulate a full experiment sequence
        instrument::record_experiment(&metrics, true);

        let start = std::time::Instant::now();
        instrument::record_action(&metrics, "tumult-db", "kill-connections", start, true);
        instrument::record_probe(&metrics, "tumult-http", "health-check", start, true);
        instrument::record_probe(&metrics, "tumult-http", "health-check", start, false);
        instrument::record_deviation(&metrics);
        instrument::record_action(&metrics, "tumult-db", "restore-pool", start, true);
        instrument::record_experiment(&metrics, false);
    }

    // ── TumultTelemetry ────────────────────────────────────────

    #[test]
    fn telemetry_disabled_config_does_not_initialize_providers() {
        let config = TelemetryConfig {
            enabled: false,
            ..TelemetryConfig::default()
        };
        let telemetry = TumultTelemetry::new(config);
        assert!(!telemetry.is_enabled());
        telemetry.shutdown(); // should not panic
    }

    #[test]
    fn telemetry_enabled_without_endpoint_does_not_panic() {
        let config = TelemetryConfig {
            enabled: true,
            otlp_endpoint: None,
            ..TelemetryConfig::default()
        };
        let telemetry = TumultTelemetry::new(config);
        assert!(telemetry.is_enabled());
        assert_eq!(telemetry.service_name(), "tumult");
        telemetry.shutdown();
    }

    #[test]
    fn telemetry_enabled_with_endpoint_initializes() {
        let rt = tokio_minimal::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = rt.enter();
        let config = TelemetryConfig {
            enabled: true,
            otlp_endpoint: Some("http://localhost:4317".into()),
            ..TelemetryConfig::default()
        };
        let telemetry = TumultTelemetry::new(config);
        assert!(telemetry.is_enabled());
        telemetry.shutdown();
    }

    #[test]
    fn telemetry_debug_trait_works() {
        let config = TelemetryConfig::default();
        let telemetry = TumultTelemetry::new(config);
        let debug = format!("{telemetry:?}");
        assert!(debug.contains("TumultTelemetry"));
    }
}
