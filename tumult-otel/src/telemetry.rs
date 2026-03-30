//! Telemetry initialization and lifecycle management.
//!
//! Initializes the OTLP exporter and TracerProvider.
//! Call `shutdown()` before process exit to flush pending telemetry.

use opentelemetry::global;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;

use crate::config::TelemetryConfig;
use opentelemetry_otlp::WithExportConfig;

/// Central telemetry manager for the Tumult platform.
#[derive(Debug)]
pub struct TumultTelemetry {
    config: TelemetryConfig,
    tracer_provider: Option<SdkTracerProvider>,
}

impl TumultTelemetry {
    /// Initialize OTel providers based on configuration.
    ///
    /// When enabled with an OTLP endpoint, sets up the gRPC exporter
    /// and installs a global tracer provider. All spans from `opentelemetry::global::tracer()`
    /// will be exported to the configured collector.
    pub fn new(config: TelemetryConfig) -> Self {
        if !config.enabled {
            return Self {
                config,
                tracer_provider: None,
            };
        }

        let resource = Resource::builder()
            .with_service_name(config.service_name.clone())
            .build();

        let provider = if let Some(ref endpoint) = config.otlp_endpoint {
            match opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint.clone())
                .build()
            {
                Ok(exporter) => {
                    let provider = SdkTracerProvider::builder()
                        .with_resource(resource)
                        .with_batch_exporter(exporter)
                        .build();
                    global::set_tracer_provider(provider.clone());
                    tracing::info!(endpoint = %endpoint, service = %config.service_name, "OTLP exporter initialized");
                    Some(provider)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to init OTLP exporter");
                    None
                }
            }
        } else {
            tracing::debug!(service = %config.service_name, "OTel enabled, no OTLP endpoint configured");
            None
        };

        Self {
            config,
            tracer_provider: provider,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
    pub fn service_name(&self) -> &str {
        &self.config.service_name
    }

    /// Flush pending telemetry and shut down providers.
    pub fn shutdown(&self) {
        if let Some(ref provider) = self.tracer_provider {
            if let Err(e) = provider.shutdown() {
                tracing::warn!(error = %e, "tracer provider shutdown error");
            }
        }
    }
}
