//! Telemetry configuration.

/// Configuration for Tumult's OpenTelemetry setup.
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub enabled: bool,
    pub service_name: String,
    pub console_export: bool,
    pub otlp_endpoint: Option<String>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            service_name: "tumult".to_string(),
            console_export: false,
            otlp_endpoint: None,
        }
    }
}

impl TelemetryConfig {
    /// Build configuration from environment variables.
    ///
    /// Reads:
    /// - `TUMULT_OTEL_ENABLED` (default: true)
    /// - `TUMULT_OTEL_CONSOLE` (default: false)
    /// - `OTEL_SERVICE_NAME` (default: "tumult")
    /// - `OTEL_EXPORTER_OTLP_ENDPOINT` (default: None)
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("TUMULT_OTEL_ENABLED")
                .map(|v| v != "false" && v != "0")
                .unwrap_or(true),
            service_name: std::env::var("OTEL_SERVICE_NAME")
                .unwrap_or_else(|_| "tumult".to_string()),
            console_export: std::env::var("TUMULT_OTEL_CONSOLE")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(false),
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
        }
    }
}
