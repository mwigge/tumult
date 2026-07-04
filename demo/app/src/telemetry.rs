//! Tracing + OpenTelemetry initialization.
//!
//! Installs a `tracing` subscriber with console output plus an `OTel` bridge
//! layer that exports spans over **OTLP HTTP** (the tumult-collector listens
//! on `:4318`, per the demo CONTRACT). If the exporter cannot be constructed
//! the app still runs with console logging only — telemetry must never take
//! the demo down.

use opentelemetry::trace::TracerProvider as _;
use opentelemetry::{global, KeyValue};
use opentelemetry_otlp::{Protocol, WithExportConfig};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

const SERVICE_NAME: &str = "demo-app";

fn env_filter() -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Initialize tracing with an OTLP/HTTP span exporter.
///
/// `endpoint` is the OTLP base (e.g. `http://tumult-collector:4318`); the
/// `/v1/traces` signal path is appended here. When set programmatically the
/// exporter uses the endpoint verbatim, so we must append it ourselves.
///
/// Returns the provider so `main` can flush spans on shutdown; `None` if the
/// exporter could not be built (console logging is still installed).
pub fn init(endpoint: &str) -> Option<SdkTracerProvider> {
    let traces_endpoint = format!("{}/v1/traces", endpoint.trim_end_matches('/'));

    let resource = Resource::builder()
        .with_service_name(SERVICE_NAME)
        .with_attribute(KeyValue::new("service.version", env!("CARGO_PKG_VERSION")))
        .build();

    match opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_endpoint(&traces_endpoint)
        .with_protocol(Protocol::HttpBinary)
        .build()
    {
        Ok(exporter) => {
            let provider = SdkTracerProvider::builder()
                .with_resource(resource)
                .with_batch_exporter(exporter)
                .build();

            global::set_tracer_provider(provider.clone());

            let otel_layer =
                tracing_opentelemetry::layer().with_tracer(provider.tracer(SERVICE_NAME));
            let _ = tracing_subscriber::registry()
                .with(env_filter())
                .with(tracing_subscriber::fmt::layer())
                .with(otel_layer)
                .try_init();

            tracing::info!(
                endpoint = %traces_endpoint,
                service = SERVICE_NAME,
                "OTLP/HTTP span exporter initialized"
            );
            Some(provider)
        }
        Err(e) => {
            let _ = tracing_subscriber::registry()
                .with(env_filter())
                .with(tracing_subscriber::fmt::layer())
                .try_init();
            tracing::warn!(error = %e, "failed to build OTLP exporter; running without trace export");
            None
        }
    }
}
