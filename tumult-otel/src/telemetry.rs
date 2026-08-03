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
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use crate::config::TelemetryConfig;
use opentelemetry_otlp::{WithExportConfig, WithTonicConfig};

const SERVICE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Parse `OTEL_EXPORTER_OTLP_HEADERS` (`k=v,k2=v2`): split on `,`, then on
/// the FIRST `=`; keys are trimmed and lowercased (tonic metadata keys are
/// lowercase); malformed pairs are skipped with a warning. This is how
/// seeds/CLIs pass `authorization=Bearer kro_...` to the ingest gRPC
/// endpoint.
fn parse_otlp_headers(raw: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for pair in raw.split(',') {
        let pair = pair.trim();
        if pair.is_empty() {
            continue;
        }
        let Some((key, value)) = pair.split_once('=') else {
            tracing::warn!(
                pair,
                "skipping malformed OTEL_EXPORTER_OTLP_HEADERS entry (want k=v)"
            );
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        if key.is_empty() {
            tracing::warn!(
                pair,
                "skipping OTEL_EXPORTER_OTLP_HEADERS entry with empty key"
            );
            continue;
        }
        pairs.push((key, value.trim().to_string()));
    }
    pairs
}

/// The `OTEL_EXPORTER_OTLP_HEADERS` env var as tonic request metadata, or
/// `None` when unset/empty. Entries that are not valid gRPC metadata (bad
/// key or value characters) are skipped with a warning.
fn otlp_headers_from_env() -> Option<tonic::metadata::MetadataMap> {
    let raw = std::env::var("OTEL_EXPORTER_OTLP_HEADERS")
        .ok()
        .filter(|v| !v.trim().is_empty())?;
    let pairs = parse_otlp_headers(&raw);
    if pairs.is_empty() {
        return None;
    }
    let mut map = tonic::metadata::MetadataMap::new();
    for (key, value) in pairs {
        if let (Ok(k), Ok(v)) = (
            tonic::metadata::MetadataKey::from_bytes(key.as_bytes()),
            value.parse::<tonic::metadata::MetadataValue<_>>(),
        ) {
            map.append(k, v);
        } else {
            tracing::warn!(
                key,
                "skipping OTEL_EXPORTER_OTLP_HEADERS entry invalid for gRPC metadata"
            );
        }
    }
    Some(map)
}

/// The gRPC request metadata the OTLP exporters should send: explicit config
/// headers win, falling back to `OTEL_EXPORTER_OTLP_HEADERS`.
fn effective_otlp_headers(
    configured: Option<&tonic::metadata::MetadataMap>,
) -> Option<tonic::metadata::MetadataMap> {
    configured.cloned().or_else(otlp_headers_from_env)
}

/// The human-readable fmt layer, directed to **stderr** — never stdout.
///
/// stdout is reserved for machine-readable output: the MCP stdio transport
/// speaks JSON-RPC there (`tumult mcp serve`, the `tumult-mcp` binary), and
/// CLI commands print command output there. Interleaved log lines would
/// corrupt the JSON-RPC stream or pollute piped command output.
fn fmt_layer<S>() -> impl tracing_subscriber::Layer<S>
where
    S: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    tracing_subscriber::fmt::layer().with_writer(std::io::stderr)
}

/// Initialize the OTLP metrics pipeline and register it as the global
/// [`SdkMeterProvider`].
///
/// Mirrors the tracer init semantics of [`TumultTelemetry::new`]:
///
/// * Returns `None` when `config.enabled` is `false`.
/// * Returns `None` when no OTLP endpoint is configured (metrics have no
///   console-export fallback — `opentelemetry-stdout` is trace-only here).
/// * On success, builds a gRPC-tonic metrics exporter for the configured
///   endpoint, wraps it in a `PeriodicReader`, registers the provider via
///   [`global::set_meter_provider`], and returns it.
/// * On exporter build failure, logs a warning and returns `None` — metrics
///   keep routing to the pre-existing (noop) global provider.
///
/// The returned provider must be kept alive for the process lifetime and shut
/// down on exit (see [`TumultTelemetry::shutdown`]); dropping it without
/// shutdown loses the final export interval.
///
/// **Note:** [`TumultTelemetry::new`] already calls this internally. Binaries
/// that use `TumultTelemetry` must NOT call this again — a second call builds
/// a second provider with its own reader loop, double-exporting every metric.
/// Call it directly only when managing providers without `TumultTelemetry`.
#[must_use = "the provider must be retained and shut down on exit"]
pub fn init_meter_provider(config: &TelemetryConfig) -> Option<SdkMeterProvider> {
    if !config.enabled {
        return None;
    }
    let endpoint = config.otlp_endpoint.as_ref()?;

    let resource = Resource::builder()
        .with_service_name(config.service_name.clone())
        .with_attribute(KeyValue::new("service.version", SERVICE_VERSION))
        .build();

    let builder = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint.as_str());
    let builder = match effective_otlp_headers(config.otlp_headers.as_ref()) {
        Some(metadata) => builder.with_metadata(metadata),
        None => builder,
    };
    match builder.build() {
        Ok(exporter) => {
            let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(exporter).build();
            let provider = SdkMeterProvider::builder()
                .with_resource(resource)
                .with_reader(reader)
                .build();
            global::set_meter_provider(provider.clone());
            tracing::info!(endpoint = %endpoint, service = %config.service_name, "OTLP metrics exporter initialized");
            Some(provider)
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to init OTLP metrics exporter");
            None
        }
    }
}

/// Initialize the OTLP logs pipeline and return its [`SdkLoggerProvider`].
///
/// Mirrors [`init_meter_provider`]: returns `None` when telemetry is
/// disabled, no OTLP endpoint is configured, or the exporter fails to build
/// (logs then stay on the local fmt layer only). On success, builds a
/// gRPC-tonic log exporter behind a batch processor. Every tracing event is
/// mirrored to the collector — stamped with the active trace/span ids — via
/// an `OpenTelemetryTracingBridge` layer on the subscriber.
///
/// The provider is not registered globally (the bridge layer holds it
/// directly); the returned provider must be kept alive for the process
/// lifetime and shut down on exit (see [`TumultTelemetry::shutdown`]).
#[must_use = "the provider must be retained and shut down on exit"]
pub fn init_logger_provider(config: &TelemetryConfig) -> Option<SdkLoggerProvider> {
    if !config.enabled {
        return None;
    }
    let endpoint = config.otlp_endpoint.as_ref()?;

    let resource = Resource::builder()
        .with_service_name(config.service_name.clone())
        .with_attribute(KeyValue::new("service.version", SERVICE_VERSION))
        .build();

    let builder = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint.as_str());
    let builder = match effective_otlp_headers(config.otlp_headers.as_ref()) {
        Some(metadata) => builder.with_metadata(metadata),
        None => builder,
    };
    match builder.build() {
        Ok(exporter) => {
            let provider = SdkLoggerProvider::builder()
                .with_resource(resource)
                .with_batch_exporter(exporter)
                .build();
            tracing::info!(endpoint = %endpoint, service = %config.service_name, "OTLP logs exporter initialized");
            Some(provider)
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to init OTLP log exporter");
            None
        }
    }
}

/// Central telemetry manager for the Tumult platform.
#[derive(Debug)]
pub struct TumultTelemetry {
    enabled: bool,
    service_name: String,
    tracer_provider: Option<SdkTracerProvider>,
    meter_provider: Option<SdkMeterProvider>,
    logger_provider: Option<SdkLoggerProvider>,
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
        // Initialize the metrics pipeline first: it borrows the config, while
        // the tracer setup below moves fields out of it. When telemetry is
        // disabled or no OTLP endpoint is configured this is `None` and every
        // `global::meter(...)` instrument stays on the noop provider.
        let meter_provider = init_meter_provider(&config);
        // Logs pipeline: batch OTLP/gRPC exporter mirrored from tracing
        // events; `None` under the same conditions as the meter provider.
        let logger_provider = init_logger_provider(&config);

        // Move service_name out of config immediately so the Resource builder
        // and the final struct both consume the owned String without cloning.
        let service_name = config.service_name;

        if !config.enabled {
            // Install a minimal tracing subscriber for log output only
            let _ = tracing_subscriber::registry()
                .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
                .with(fmt_layer())
                .try_init();
            return Self {
                enabled: false,
                service_name,
                tracer_provider: None,
                meter_provider,
                logger_provider,
            };
        }

        let resource = Resource::builder()
            .with_service_name(service_name.clone())
            .with_attribute(KeyValue::new("service.version", SERVICE_VERSION))
            .build();

        // Move the endpoint out of the Option so it can be passed by value to
        // `with_endpoint`, avoiding a `.clone()` on the full String.
        let provider = if let Some(endpoint) = config.otlp_endpoint {
            let builder = opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_endpoint(endpoint.as_str());
            let builder = match effective_otlp_headers(config.otlp_headers.as_ref()) {
                Some(metadata) => builder.with_metadata(metadata),
                None => builder,
            };
            match builder.build() {
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

                    // Step 2: Install tracing subscriber with OTel bridge layers
                    // (traces via tracing-opentelemetry, logs via the appender).
                    let otel_layer = tracing_opentelemetry::layer();
                    let log_bridge = logger_provider.as_ref().map(
                        opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new,
                    );
                    let _ = tracing_subscriber::registry()
                        .with(
                            EnvFilter::try_from_default_env()
                                .unwrap_or_else(|_| EnvFilter::new("info")),
                        )
                        .with(fmt_layer())
                        .with(otel_layer)
                        .with(log_bridge)
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
                        .with(fmt_layer())
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
                .with(fmt_layer())
                .with(otel_layer)
                .try_init();

            tracing::debug!(service = %service_name, "console-only span export enabled");
            Some(provider)
        } else {
            // Install subscriber without OTel layer
            let _ = tracing_subscriber::registry()
                .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
                .with(fmt_layer())
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
            meter_provider,
            logger_provider,
        }
    }

    /// Whether telemetry is actively exporting — i.e. a tracer provider was
    /// successfully built. `false` when disabled in config, when no exporter
    /// was configured, or when provider initialization failed.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// The service name stamped on the `OTel` resource (`service.name`).
    #[must_use]
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Flush pending telemetry and shut down providers.
    ///
    /// Shuts down the locally-held `SdkTracerProvider` and `SdkMeterProvider`
    /// clones **and** replaces both globally registered providers with noop
    /// implementations.
    ///
    /// Without resetting the globals, any spans or metric recordings emitted
    /// after this call would be routed to already-closed exporters, causing
    /// silent drops or error-log storms depending on the exporter
    /// implementation. `SdkMeterProvider::shutdown` flushes pending metric
    /// data points before closing its reader.
    ///
    /// Idempotent: repeated calls log a warning from the SDK's second
    /// shutdown attempt but never panic.
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

        if let Some(ref provider) = self.meter_provider {
            if let Err(e) = provider.shutdown() {
                tracing::warn!(error = %e, "meter provider shutdown error");
            }
        }
        // Same rationale as the tracer reset above: post-shutdown metric
        // recordings must hit a noop provider, not a closed exporter.
        global::set_meter_provider(opentelemetry::metrics::noop::NoopMeterProvider::new());

        // The log bridge layer holds this provider directly (no global), so
        // events emitted after shutdown are dropped by the SDK's shut-down
        // batch processor rather than exported.
        if let Some(ref provider) = self.logger_provider {
            if let Err(e) = provider.shutdown() {
                tracing::warn!(error = %e, "logger provider shutdown error");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        effective_otlp_headers, init_logger_provider, init_meter_provider, otlp_headers_from_env,
        parse_otlp_headers, TelemetryConfig,
    };

    /// Serialises the test that mutates `OTEL_EXPORTER_OTLP_HEADERS` so no
    /// other test thread observes a half-configured value.
    static HEADER_ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn current_thread_runtime() -> tokio_minimal::runtime::Runtime {
        tokio_minimal::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    #[test]
    fn parses_standard_pairs() {
        assert_eq!(
            parse_otlp_headers("authorization=Bearer kro_abc,x-tenant=dev"),
            owned(&[("authorization", "Bearer kro_abc"), ("x-tenant", "dev")])
        );
    }

    #[test]
    fn splits_on_first_equals_only() {
        assert_eq!(parse_otlp_headers("key=a=b=c"), owned(&[("key", "a=b=c")]));
    }

    #[test]
    fn trims_whitespace_and_lowercases_keys() {
        assert_eq!(
            parse_otlp_headers("  Authorization = Bearer x  , X-Tenant= dev"),
            owned(&[("authorization", "Bearer x"), ("x-tenant", "dev")])
        );
    }

    #[test]
    fn skips_malformed_pairs() {
        assert_eq!(
            parse_otlp_headers("no-equals-here,=empty-key,ok=1"),
            owned(&[("ok", "1")])
        );
    }

    #[test]
    fn empty_input_yields_nothing() {
        assert!(parse_otlp_headers("").is_empty());
        assert!(parse_otlp_headers("  , ,").is_empty());
    }

    #[test]
    fn explicit_config_headers_take_precedence() {
        let mut map = tonic::metadata::MetadataMap::new();
        map.insert("authorization", "Bearer kro_test".parse().unwrap());
        let resolved = effective_otlp_headers(Some(&map)).unwrap();
        assert_eq!(resolved.get("authorization").unwrap(), "Bearer kro_test");
    }

    #[test]
    fn otlp_headers_from_env_scenarios() {
        let _lock = HEADER_ENV_MUTEX.lock().unwrap();
        let prev = std::env::var("OTEL_EXPORTER_OTLP_HEADERS").ok();

        std::env::remove_var("OTEL_EXPORTER_OTLP_HEADERS");
        assert!(otlp_headers_from_env().is_none());
        assert!(effective_otlp_headers(None).is_none());

        std::env::set_var("OTEL_EXPORTER_OTLP_HEADERS", "   ");
        assert!(otlp_headers_from_env().is_none());

        std::env::set_var("OTEL_EXPORTER_OTLP_HEADERS", "no-equals,=empty-key");
        assert!(otlp_headers_from_env().is_none());

        std::env::set_var(
            "OTEL_EXPORTER_OTLP_HEADERS",
            "authorization=Bearer kro_env,x-tenant=dev",
        );
        let map = otlp_headers_from_env().expect("headers");
        assert_eq!(map.get("authorization").unwrap(), "Bearer kro_env");
        assert_eq!(map.get("x-tenant").unwrap(), "dev");
        // With no explicit config headers, the env var is the fallback.
        let resolved = effective_otlp_headers(None).expect("headers");
        assert_eq!(resolved.get("authorization").unwrap(), "Bearer kro_env");

        // Entries that are not valid gRPC metadata are skipped; valid ones stay.
        std::env::set_var("OTEL_EXPORTER_OTLP_HEADERS", "sp ace=v,ok=1");
        let map = otlp_headers_from_env().expect("headers");
        assert_eq!(map.get("ok").unwrap(), "1");
        assert!(map.get("sp ace").is_none());

        match prev {
            Some(value) => std::env::set_var("OTEL_EXPORTER_OTLP_HEADERS", value),
            None => std::env::remove_var("OTEL_EXPORTER_OTLP_HEADERS"),
        }
    }

    #[test]
    fn init_logger_provider_returns_none_when_disabled_or_without_endpoint() {
        let disabled = TelemetryConfig {
            enabled: false,
            otlp_endpoint: Some("http://localhost:4317".into()),
            ..TelemetryConfig::default()
        };
        assert!(init_logger_provider(&disabled).is_none());

        let no_endpoint = TelemetryConfig {
            enabled: true,
            otlp_endpoint: None,
            ..TelemetryConfig::default()
        };
        assert!(init_logger_provider(&no_endpoint).is_none());
    }

    #[test]
    fn init_logger_provider_builds_provider_with_explicit_headers() {
        let rt = current_thread_runtime();
        let _guard = rt.enter();
        let mut headers = tonic::metadata::MetadataMap::new();
        headers.insert("authorization", "Bearer kro_cfg".parse().unwrap());
        let config = TelemetryConfig {
            enabled: true,
            otlp_endpoint: Some("http://localhost:4317".into()),
            otlp_headers: Some(headers),
            ..TelemetryConfig::default()
        };
        let provider = init_logger_provider(&config).expect("provider");
        assert!(provider.shutdown().is_ok());
    }

    #[test]
    fn exporter_builders_return_none_for_unparseable_endpoints() {
        let rt = current_thread_runtime();
        let _guard = rt.enter();
        // The build error must surface as `None` (with a warning), not a panic.
        let config = TelemetryConfig {
            enabled: true,
            otlp_endpoint: Some("http://[::1".into()),
            ..TelemetryConfig::default()
        };
        assert!(init_meter_provider(&config).is_none());
        assert!(init_logger_provider(&config).is_none());
    }
}
