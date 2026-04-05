//! Telemetry initialization and lifecycle management.
//!
//! Initializes the OTLP exporter and `TracerProvider`, then installs
//! a tracing subscriber with an OpenTelemetry bridge layer.
//!
//! **Init order** (per `OTel` spec): `TracerProvider` is registered as
//! global BEFORE the tracing subscriber is installed. This ensures
//! the bridge layer can resolve a valid provider immediately.
//!
//! Call `shutdown()` before process exit to flush pending telemetry.

use opentelemetry::global;
use opentelemetry::KeyValue;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use crate::config::TelemetryConfig;
use opentelemetry_otlp::WithExportConfig;

const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Central telemetry manager for the Tumult platform.
#[derive(Debug)]
pub struct TumultTelemetry {
    enabled: bool,
    service_name: String,
    tracer_provider: Option<SdkTracerProvider>,
}

impl TumultTelemetry {
    /// Initialize `OTel` providers based on configuration.
    ///
    /// When enabled with an OTLP endpoint, sets up the gRPC exporter
    /// and installs a global tracer provider. The tracing subscriber
    /// with OpenTelemetry bridge is installed **after** the provider
    /// is registered globally, ensuring correct init order.
    ///
    /// When `config.console_export` is `true`, span data is also written
    /// to stdout in addition to any configured OTLP endpoint. This is
    /// useful for local development and debugging.
    pub fn new(config: TelemetryConfig) -> Self {
        // Move service_name out of config immediately so the Resource builder
        // and the final struct both consume the owned String without cloning.
        let service_name = config.service_name;

        if !config.enabled {
            // Install a minimal tracing subscriber for log output only
            let _ = tracing_subscriber::registry()
                .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
                .with(tracing_subscriber::fmt::layer())
                .try_init();
            return Self {
                enabled: false,
                service_name,
                tracer_provider: None,
            };
        }

        let resource = Resource::builder()
            .with_service_name(service_name.clone())
            .with_attribute(KeyValue::new("service.version", SERVICE_VERSION))
            .build();

        // Move the endpoint out of the Option so it can be passed by value to
        // `with_endpoint`, avoiding a `.clone()` on the full String.
        let provider = if let Some(endpoint) = config.otlp_endpoint {
            match opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint.as_str())
                .build()
            {
                Ok(exporter) => {
                    let mut builder = SdkTracerProvider::builder()
                        .with_resource(resource)
                        .with_batch_exporter(exporter);

                    if config.console_export {
                        let stdout_exporter = opentelemetry_stdout::SpanExporter::default();
                        builder = builder.with_simple_exporter(stdout_exporter);
                        tracing::debug!("console span export enabled");
                    }

                    let provider = builder.build();

                    // Step 1: Register TracerProvider BEFORE installing subscriber
                    global::set_tracer_provider(provider.clone());

                    // Step 2: Install tracing subscriber with OTel bridge layer
                    let otel_layer = tracing_opentelemetry::layer();
                    let _ = tracing_subscriber::registry()
                        .with(
                            EnvFilter::try_from_default_env()
                                .unwrap_or_else(|_| EnvFilter::new("info")),
                        )
                        .with(tracing_subscriber::fmt::layer())
                        .with(otel_layer)
                        .try_init();

                    tracing::info!(endpoint = %endpoint, service = %service_name, "OTLP exporter initialized");
                    Some(provider)
                }
                Err(e) => {
                    // Install subscriber without OTel layer on failure
                    let _ = tracing_subscriber::registry()
                        .with(
                            EnvFilter::try_from_default_env()
                                .unwrap_or_else(|_| EnvFilter::new("info")),
                        )
                        .with(tracing_subscriber::fmt::layer())
                        .try_init();
                    tracing::warn!(error = %e, "failed to init OTLP exporter");
                    None
                }
            }
        } else if config.console_export {
            // No OTLP endpoint but console export requested: build a provider
            // that writes spans to stdout only.
            let stdout_exporter = opentelemetry_stdout::SpanExporter::default();
            let provider = SdkTracerProvider::builder()
                .with_resource(resource)
                .with_simple_exporter(stdout_exporter)
                .build();

            global::set_tracer_provider(provider.clone());

            let otel_layer = tracing_opentelemetry::layer();
            let _ = tracing_subscriber::registry()
                .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
                .with(tracing_subscriber::fmt::layer())
                .with(otel_layer)
                .try_init();

            tracing::debug!(service = %service_name, "console-only span export enabled");
            Some(provider)
        } else {
            // Install subscriber without OTel layer
            let _ = tracing_subscriber::registry()
                .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
                .with(tracing_subscriber::fmt::layer())
                .try_init();
            tracing::debug!(service = %service_name, "OTel enabled, no OTLP endpoint configured");
            None
        };

        Self {
            // Only mark telemetry enabled when a provider was successfully built.
            // An OTLP build failure leaves `provider = None`; reporting `enabled =
            // true` in that state would mislead callers into believing spans are
            // being exported when they are silently dropped.
            enabled: config.enabled && provider.is_some(),
            service_name,
            tracer_provider: provider,
        }
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    #[must_use]
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Flush pending telemetry and shut down providers.
    ///
    /// Shuts down the locally-held `SdkTracerProvider` clone **and** replaces
    /// the globally registered provider with a `NoopTracerProvider`.
    ///
    /// Without resetting the global, any spans emitted after this call would be
    /// routed to an already-closed exporter, causing silent drops or error-log
    /// storms depending on the exporter implementation.
    pub fn shutdown(&self) {
        if let Some(ref provider) = self.tracer_provider {
            if let Err(e) = provider.shutdown() {
                tracing::warn!(error = %e, "tracer provider shutdown error");
            }
        }
        // Replace the global provider with a noop so that spans emitted after
        // shutdown are silently discarded rather than routed to a dead exporter.
        // This is a no-op in tests or when OTel was never configured.
        global::set_tracer_provider(opentelemetry::trace::noop::NoopTracerProvider::new());
    }
}
