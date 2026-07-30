//! Telemetry configuration.

const DEFAULT_SERVICE_NAME: &str = "tumult";

/// Configuration for Tumult's OpenTelemetry setup.
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub enabled: bool,
    pub service_name: String,
    pub console_export: bool,
    pub otlp_endpoint: Option<String>,
    /// Explicit gRPC request metadata for the OTLP exporters (e.g. an
    /// `authorization` header). When set, this takes precedence over the
    /// `OTEL_EXPORTER_OTLP_HEADERS` environment variable — used by tumultd to
    /// authenticate its telemetry loopback against its own token-guarded
    /// ingest without mutating the process environment.
    pub otlp_headers: Option<tonic::metadata::MetadataMap>,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            service_name: DEFAULT_SERVICE_NAME.to_string(),
            console_export: false,
            otlp_endpoint: None,
            otlp_headers: None,
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
    ///
    /// `otlp_headers` starts `None`; the `OTEL_EXPORTER_OTLP_HEADERS` env var
    /// is read by the exporter builders themselves (explicit config wins).
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            enabled: std::env::var("TUMULT_OTEL_ENABLED")
                .map_or(true, |v| v != "false" && v != "0"),
            service_name: std::env::var("OTEL_SERVICE_NAME")
                .unwrap_or_else(|_| DEFAULT_SERVICE_NAME.to_string()),
            console_export: std::env::var("TUMULT_OTEL_CONSOLE")
                .is_ok_and(|v| v == "true" || v == "1"),
            otlp_endpoint: std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok(),
            otlp_headers: None,
        }
    }
}
