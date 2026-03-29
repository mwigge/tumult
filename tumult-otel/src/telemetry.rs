//! Telemetry initialization and lifecycle management.

use crate::config::TelemetryConfig;

/// Central telemetry manager for the Tumult platform.
///
/// Initializes OpenTelemetry tracer, meter, and logger providers.
/// Call `shutdown()` before process exit to flush pending telemetry.
pub struct TumultTelemetry {
    config: TelemetryConfig,
}

impl TumultTelemetry {
    /// Create a new telemetry instance.
    ///
    /// If `config.enabled` is false, no providers are initialized
    /// and all instrumentation becomes a no-op.
    pub fn new(config: TelemetryConfig) -> Self {
        if config.enabled {
            // Provider initialization will be implemented when
            // the engine integrates with the OTel crate.
            // For now, the global tracer/meter/logger are available
            // via opentelemetry::global.
            tracing::debug!(
                service_name = %config.service_name,
                "telemetry initialized"
            );
        }
        Self { config }
    }

    /// Whether telemetry is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Get the service name.
    pub fn service_name(&self) -> &str {
        &self.config.service_name
    }

    /// Flush all pending telemetry and shut down providers.
    ///
    /// Must be called before process exit to ensure all spans
    /// and metrics are exported. Provider-level shutdown will be
    /// implemented when OTLP exporters are wired up.
    pub fn shutdown(&self) {
        if self.config.enabled {
            tracing::debug!("telemetry shut down");
        }
    }
}
