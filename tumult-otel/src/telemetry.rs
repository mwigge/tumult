//! Telemetry initialization and lifecycle management.
//!
//! Builds the OTLP trace, metric and log pipelines and registers the tracer
//! and meter providers globally.
//!
//! This module does **not** install a `tracing` subscriber. That is done
//! once, at process start, by [`logging::init`](crate::logging::init).
//! [`TumultTelemetry::new`] hands its logger provider to the OTLP log bridge
//! that subscriber already contains, so events are mirrored to the collector
//! from that point on.
//!
//! Call `shutdown()` before process exit to flush pending telemetry.

use opentelemetry::global;
use opentelemetry::KeyValue;
use opentelemetry_otlp::{WithExportConfig, WithTonicConfig};
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::Resource;
use tonic::metadata::{AsciiMetadataKey, AsciiMetadataValue, MetadataMap};

use crate::config::TelemetryConfig;
use crate::logging::{self, ENVIRONMENT_ATTRIBUTE, SERVICE_VERSION};

/// Environment variable holding extra OTLP request headers (`k=v,k2=v2`).
const HEADERS_ENV: &str = "OTEL_EXPORTER_OTLP_HEADERS";

const SIGNAL_TRACES: &str = "traces";
const SIGNAL_METRICS: &str = "metrics";
const SIGNAL_LOGS: &str = "logs";

/// One well-formed `key=value` entry of the header list.
struct HeaderEntry {
    /// 1-based position in the comma-separated list, for diagnostics.
    index: usize,
    key: String,
    value: String,
}

/// Parse an `OTEL_EXPORTER_OTLP_HEADERS` value (`k=v,k2=v2`): split on `,`,
/// then on the FIRST `=`; keys are trimmed and lowercased (tonic metadata
/// keys are lowercase); malformed entries are skipped with a warning. This
/// is how seeds/CLIs pass `authorization=Bearer kro_...` to the ingest gRPC
/// endpoint.
///
/// Header values are credentials. A skipped entry is identified by its
/// position only: a malformed entry has no reliable key/value boundary, so
/// no part of it is safe to log.
fn parse_otlp_headers(raw: &str) -> Vec<HeaderEntry> {
    raw.split(',')
        .enumerate()
        .filter_map(|(position, entry)| parse_header_entry(position + 1, entry.trim()))
        .collect()
}

fn parse_header_entry(index: usize, entry: &str) -> Option<HeaderEntry> {
    if entry.is_empty() {
        return None;
    }
    let Some((key, value)) = entry.split_once('=') else {
        tracing::warn!(
            name: "otel.exporter.header.malformed",
            { entry.index = index },
            "skipping malformed OTEL_EXPORTER_OTLP_HEADERS entry; expected key=value"
        );
        return None;
    };
    let key = key.trim().to_ascii_lowercase();
    if key.is_empty() {
        tracing::warn!(
            name: "otel.exporter.header.empty_key",
            { entry.index = index },
            "skipping OTEL_EXPORTER_OTLP_HEADERS entry with an empty key"
        );
        return None;
    }
    Some(HeaderEntry {
        index,
        key,
        value: value.trim().to_string(),
    })
}

/// Converts an entry to gRPC metadata, or logs why it cannot be sent.
fn metadata_entry(entry: &HeaderEntry) -> Option<(AsciiMetadataKey, AsciiMetadataValue)> {
    // A string that is not a valid metadata key is usually a mistyped entry
    // (`authorization: Bearer ...`), so it may contain the credential.
    let Ok(key) = AsciiMetadataKey::from_bytes(entry.key.as_bytes()) else {
        tracing::warn!(
            name: "otel.exporter.header.invalid_key",
            { entry.index = entry.index },
            "skipping OTEL_EXPORTER_OTLP_HEADERS entry whose key is not valid gRPC metadata"
        );
        return None;
    };
    let Ok(value) = entry.value.parse::<AsciiMetadataValue>() else {
        tracing::warn!(
            name: "otel.exporter.header.invalid_value",
            { entry.index = entry.index, config.key = key.as_str() },
            "skipping OTEL_EXPORTER_OTLP_HEADERS entry whose value is not valid gRPC metadata"
        );
        return None;
    };
    Some((key, value))
}

/// `raw` (the `OTEL_EXPORTER_OTLP_HEADERS` format) as tonic request
/// metadata, or `None` when it holds no usable entry.
fn otlp_headers_from(raw: &str) -> Option<MetadataMap> {
    let mut map = MetadataMap::new();
    for entry in parse_otlp_headers(raw) {
        if let Some((key, value)) = metadata_entry(&entry) {
            map.append(key, value);
        }
    }
    if map.is_empty() {
        return None;
    }
    Some(map)
}

/// The `OTEL_EXPORTER_OTLP_HEADERS` env var as tonic request metadata, or
/// `None` when unset/empty or when no entry is usable.
fn otlp_headers_from_env() -> Option<MetadataMap> {
    let raw = std::env::var(HEADERS_ENV).ok()?;
    otlp_headers_from(&raw)
}

/// The gRPC request metadata the OTLP exporters should send: explicit config
/// headers win, falling back to `OTEL_EXPORTER_OTLP_HEADERS`.
fn effective_otlp_headers(configured: Option<&MetadataMap>) -> Option<MetadataMap> {
    configured.cloned().or_else(otlp_headers_from_env)
}

/// Adds the effective request headers to an OTLP exporter builder.
fn with_headers<B: WithTonicConfig>(builder: B, config: &TelemetryConfig) -> B {
    match effective_otlp_headers(config.otlp_headers.as_ref()) {
        Some(metadata) => builder.with_metadata(metadata),
        None => builder,
    }
}

/// The resource every signal is stamped with. The same three attributes are
/// the top-level `service.name`, `service.version` and
/// `deployment.environment.name` keys of each JSON log line.
fn resource(service_name: &str, environment: Option<&str>) -> Resource {
    let builder = Resource::builder()
        .with_service_name(service_name.to_string())
        .with_attribute(KeyValue::new("service.version", SERVICE_VERSION));
    let Some(environment) = environment else {
        return builder.build();
    };
    builder
        .with_attribute(KeyValue::new(
            ENVIRONMENT_ATTRIBUTE,
            environment.to_string(),
        ))
        .build()
}

fn process_resource(config: &TelemetryConfig) -> Resource {
    resource(
        &config.service_name,
        logging::deployment_environment().as_deref(),
    )
}

/// Records that the exporter for `signal` is up. The endpoint is logged
/// without userinfo or query string: both can carry credentials.
fn log_exporter_initialized(signal: &'static str, endpoint: &str) {
    tracing::info!(
        name: "otel.exporter.initialized",
        { otel.signal = signal, url.full = %logging::sanitize_url(endpoint) },
        "exporter initialized"
    );
}

/// Records that the exporter for `signal` could not be built. The error
/// text of an invalid endpoint quotes the endpoint, so it is redacted too.
fn log_exporter_failed<E: std::fmt::Display>(signal: &'static str, endpoint: &str, error: &E) {
    tracing::warn!(
        name: "otel.exporter.failed",
        {
            otel.signal = signal,
            url.full = %logging::sanitize_url(endpoint),
            error.message = %logging::redact_url(&error.to_string(), endpoint),
            "error.type" = logging::error_type(error),
        },
        "exporter initialization failed; this signal is not exported"
    );
}

fn log_shutdown_failed<E: std::fmt::Display>(signal: &'static str, error: &E) {
    tracing::warn!(
        name: "otel.provider.shutdown.failed",
        {
            otel.signal = signal,
            error.message = %logging::err(error),
            "error.type" = logging::error_type(error),
        },
        "provider shutdown failed; buffered telemetry may be lost"
    );
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
    let endpoint = config.otlp_endpoint.as_deref()?;
    let builder = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint);
    let exporter = match with_headers(builder, config).build() {
        Ok(exporter) => exporter,
        Err(error) => {
            log_exporter_failed(SIGNAL_METRICS, endpoint, &error);
            return None;
        }
    };
    let reader = opentelemetry_sdk::metrics::PeriodicReader::builder(exporter).build();
    let provider = SdkMeterProvider::builder()
        .with_resource(process_resource(config))
        .with_reader(reader)
        .build();
    global::set_meter_provider(provider.clone());
    log_exporter_initialized(SIGNAL_METRICS, endpoint);
    Some(provider)
}

/// Initialize the OTLP logs pipeline and return its [`SdkLoggerProvider`].
///
/// Mirrors [`init_meter_provider`]: returns `None` when telemetry is
/// disabled, no OTLP endpoint is configured, or the exporter fails to build
/// (logs then stay on stderr only). On success, builds a gRPC-tonic log
/// exporter behind a batch processor.
///
/// The provider is not registered globally and exports nothing by itself:
/// [`TumultTelemetry::new`] attaches it to the log bridge of the subscriber
/// installed by [`logging::init`](crate::logging::init), which mirrors every
/// tracing event — stamped with the active trace/span ids — to it. The
/// returned provider must be kept alive for the process lifetime and shut
/// down on exit (see [`TumultTelemetry::shutdown`]).
#[must_use = "the provider must be retained and shut down on exit"]
pub fn init_logger_provider(config: &TelemetryConfig) -> Option<SdkLoggerProvider> {
    if !config.enabled {
        return None;
    }
    let endpoint = config.otlp_endpoint.as_deref()?;
    let builder = opentelemetry_otlp::LogExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint);
    let exporter = match with_headers(builder, config).build() {
        Ok(exporter) => exporter,
        Err(error) => {
            log_exporter_failed(SIGNAL_LOGS, endpoint, &error);
            return None;
        }
    };
    let provider = SdkLoggerProvider::builder()
        .with_resource(process_resource(config))
        .with_batch_exporter(exporter)
        .build();
    log_exporter_initialized(SIGNAL_LOGS, endpoint);
    Some(provider)
}

/// Builds the tracer provider `config` asks for and registers it globally.
/// `None` when telemetry is disabled, nothing is configured to receive
/// spans, or the OTLP exporter fails to build.
fn init_tracer_provider(config: &TelemetryConfig) -> Option<SdkTracerProvider> {
    if !config.enabled {
        return None;
    }
    let provider = match config.otlp_endpoint.as_deref() {
        Some(endpoint) => otlp_tracer_provider(config, endpoint)?,
        None if config.console_export => console_tracer_provider(config),
        None => {
            tracing::debug!(
                name: "otel.exporter.unconfigured",
                "telemetry is enabled without an OTLP endpoint; spans are not exported"
            );
            return None;
        }
    };
    global::set_tracer_provider(provider.clone());
    Some(provider)
}

/// A tracer provider exporting over OTLP/gRPC, and to stdout as well when
/// `config.console_export` is set.
fn otlp_tracer_provider(config: &TelemetryConfig, endpoint: &str) -> Option<SdkTracerProvider> {
    let builder = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint);
    let exporter = match with_headers(builder, config).build() {
        Ok(exporter) => exporter,
        Err(error) => {
            log_exporter_failed(SIGNAL_TRACES, endpoint, &error);
            return None;
        }
    };
    let mut builder = SdkTracerProvider::builder()
        .with_resource(process_resource(config))
        .with_batch_exporter(exporter);
    if config.console_export {
        builder = builder.with_simple_exporter(opentelemetry_stdout::SpanExporter::default());
        log_console_export_enabled();
    }
    log_exporter_initialized(SIGNAL_TRACES, endpoint);
    Some(builder.build())
}

/// A tracer provider that writes spans to stdout only.
fn console_tracer_provider(config: &TelemetryConfig) -> SdkTracerProvider {
    log_console_export_enabled();
    SdkTracerProvider::builder()
        .with_resource(process_resource(config))
        .with_simple_exporter(opentelemetry_stdout::SpanExporter::default())
        .build()
}

fn log_console_export_enabled() {
    tracing::debug!(
        name: "otel.traces.console.enabled",
        "console span export enabled; spans are written to stdout"
    );
}

/// Points the installed subscriber's OTLP log bridge at `provider`.
///
/// Failing to attach is not fatal — logs still go to stderr — but it must
/// not be silent, because the operator expects them in the collector.
fn attach_log_bridge(provider: &SdkLoggerProvider) {
    let Err(error) = logging::attach_log_bridge(provider) else {
        return;
    };
    tracing::warn!(
        name: "otel.logs.bridge.unattached",
        { error.message = %error, "error.type" = logging::error_type(&error) },
        "log events are not exported over OTLP"
    );
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
    /// When enabled with an OTLP endpoint, sets up the gRPC exporters,
    /// installs the global tracer and meter providers, and starts mirroring
    /// log events to the collector through the subscriber installed by
    /// [`logging::init`](crate::logging::init). It never installs a
    /// subscriber itself: call `logging::init` first, at process start.
    ///
    /// When `config.console_export` is `true`, span data is also written
    /// to stdout in addition to any configured OTLP endpoint. This is
    /// useful for local development and debugging.
    #[must_use = "the providers must be retained and shut down on exit"]
    pub fn new(config: TelemetryConfig) -> Self {
        // `None` when telemetry is disabled or no OTLP endpoint is
        // configured; every `global::meter(...)` instrument then stays on
        // the noop provider.
        let meter_provider = init_meter_provider(&config);
        let logger_provider = init_logger_provider(&config);
        if let Some(provider) = &logger_provider {
            attach_log_bridge(provider);
        }
        let tracer_provider = init_tracer_provider(&config);
        Self {
            // Only report telemetry as enabled when a tracer provider was
            // actually built. An OTLP build failure leaves it `None`;
            // claiming `enabled` then would mislead callers into believing
            // spans are exported when they are dropped.
            enabled: tracer_provider.is_some(),
            service_name: config.service_name,
            tracer_provider,
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
            if let Err(error) = provider.shutdown() {
                log_shutdown_failed(SIGNAL_TRACES, &error);
            }
        }
        // Replace the global provider with a noop so that spans emitted after
        // shutdown are silently discarded rather than routed to a dead exporter.
        // This is a no-op in tests or when OTel was never configured.
        global::set_tracer_provider(opentelemetry::trace::noop::NoopTracerProvider::new());

        if let Some(ref provider) = self.meter_provider {
            if let Err(error) = provider.shutdown() {
                log_shutdown_failed(SIGNAL_METRICS, &error);
            }
        }
        // Same rationale as the tracer reset above: post-shutdown metric
        // recordings must hit a noop provider, not a closed exporter.
        global::set_meter_provider(opentelemetry::metrics::noop::NoopMeterProvider::new());

        // The log bridge holds this provider directly (no global). Detach it
        // first: the SDK warns about every record emitted to a provider that
        // is shut down. Later events still reach stderr.
        if let Some(ref provider) = self.logger_provider {
            logging::detach_log_bridge();
            if let Err(error) = provider.shutdown() {
                log_shutdown_failed(SIGNAL_LOGS, &error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        effective_otlp_headers, init_logger_provider, init_meter_provider, otlp_headers_from_env,
        TelemetryConfig,
    };
    use tumult_test_utils::EnvGuard;

    /// The `(key, value)` pairs `raw` parses to, in order.
    fn parse_otlp_headers(raw: &str) -> Vec<(String, String)> {
        super::parse_otlp_headers(raw)
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect()
    }

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
        const HEADERS: &str = "OTEL_EXPORTER_OTLP_HEADERS";
        let mut env = EnvGuard::lock();

        env.remove(HEADERS);
        assert!(otlp_headers_from_env().is_none());
        assert!(effective_otlp_headers(None).is_none());

        env.set(HEADERS, "   ");
        assert!(otlp_headers_from_env().is_none());

        env.set(HEADERS, "no-equals,=empty-key");
        assert!(otlp_headers_from_env().is_none());

        env.set(HEADERS, "authorization=Bearer kro_env,x-tenant=dev");
        let map = otlp_headers_from_env().expect("headers");
        assert_eq!(map.get("authorization").unwrap(), "Bearer kro_env");
        assert_eq!(map.get("x-tenant").unwrap(), "dev");
        // With no explicit config headers, the env var is the fallback.
        let resolved = effective_otlp_headers(None).expect("headers");
        assert_eq!(resolved.get("authorization").unwrap(), "Bearer kro_env");

        // Entries that are not valid gRPC metadata are skipped; valid ones stay.
        env.set(HEADERS, "sp ace=v,ok=1");
        let map = otlp_headers_from_env().expect("headers");
        assert_eq!(map.get("ok").unwrap(), "1");
        assert!(map.get("sp ace").is_none());
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

    // ── Credential hygiene and structured events ─────────────────────────

    use crate::logging::testing::capture_json;
    use opentelemetry::Key;
    use tracing::Level;

    /// Every credential-looking fragment planted in the header fixtures.
    const PLANTED_SECRETS: [&str; 7] = [
        "kro_colon",
        "kro_emptykey",
        "a2V5",
        "tail",
        "Bearer",
        "bearer",
        "kro_badvalue",
    ];

    #[test]
    fn header_parse_warnings_never_contain_the_header_value() {
        // Four ways to get a header wrong, each with a credential in it:
        // a colon instead of `=`, an empty key, a colon form whose base64
        // token contains `=`, and a value that is not valid gRPC metadata.
        let raw = "authorization: Bearer kro_colon,=kro_emptykey,\
                   authorization: Bearer a2V5=tail,x-ok=kro_badvalue\u{1},good=1";
        let mut headers = None;
        let captured = capture_json(Level::INFO, || headers = super::otlp_headers_from(raw));

        assert_eq!(captured.events.len(), 4, "{}", captured.raw);
        for secret in PLANTED_SECRETS {
            assert!(
                !captured.raw.contains(secret),
                "{secret} leaked into: {}",
                captured.raw
            );
        }
        let headers = headers.expect("the valid entry survives");
        assert_eq!(headers.get("good").unwrap(), "1");
        assert_eq!(headers.len(), 1);
    }

    #[test]
    fn header_parse_warnings_identify_the_entry_by_position_and_key() {
        let raw = "authorization: Bearer kro_colon,=kro_emptykey,\
                   authorization: Bearer a2V5=tail,x-ok=kro_badvalue\u{1},good=1";
        let captured = capture_json(Level::INFO, || drop(super::otlp_headers_from(raw)));

        let names: Vec<&str> = captured
            .events
            .iter()
            .map(|event| event["event.name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "otel.exporter.header.malformed",
                "otel.exporter.header.empty_key",
                "otel.exporter.header.invalid_key",
                "otel.exporter.header.invalid_value",
            ]
        );
        let indexes: Vec<u64> = captured
            .events
            .iter()
            .map(|event| event["entry.index"].as_u64().unwrap())
            .collect();
        assert_eq!(indexes, [1, 2, 3, 4]);
        // Only a key that is itself valid metadata is safe to name.
        assert_eq!(captured.events[3]["config.key"], "x-ok");
        for event in &captured.events[..3] {
            assert!(event.get("config.key").is_none(), "{event}");
        }
    }

    #[test]
    fn exporter_events_log_the_endpoint_without_credentials() {
        // The exporters read `OTEL_EXPORTER_OTLP_HEADERS`; hold the lock so
        // the test that mutates it cannot add header warnings to the capture.
        let _env = EnvGuard::lock();
        // A successful meter init replaces the global meter provider, which
        // other tests assert exact counter totals against.
        let _meter_lock = crate::tests::METER_MUTEX.lock().unwrap();
        let rt = current_thread_runtime();
        let _guard = rt.enter();
        let config = TelemetryConfig {
            enabled: true,
            otlp_endpoint: Some("http://alice:hunter2@localhost:4317/?token=s3cr3t".into()),
            ..TelemetryConfig::default()
        };
        let captured = capture_json(Level::INFO, || {
            let logs = init_logger_provider(&config).expect("logger provider");
            let metrics = init_meter_provider(&config).expect("meter provider");
            assert!(logs.shutdown().is_ok());
            assert!(metrics.shutdown().is_ok());
        });

        assert!(!captured.raw.contains("hunter2"), "{}", captured.raw);
        assert!(!captured.raw.contains("s3cr3t"), "{}", captured.raw);
        let initialized: Vec<_> = captured
            .events
            .iter()
            .filter(|event| {
                event["message"]
                    .as_str()
                    .unwrap()
                    .ends_with("exporter initialized")
            })
            .collect();
        assert_eq!(initialized.len(), 2, "{}", captured.raw);
        for event in initialized {
            assert_eq!(event["url.full"], "http://localhost:4317/");
        }
    }

    #[test]
    fn exporter_build_failure_does_not_echo_endpoint_credentials() {
        let _env = EnvGuard::lock();
        let rt = current_thread_runtime();
        let _guard = rt.enter();
        let config = TelemetryConfig {
            enabled: true,
            otlp_endpoint: Some("http://alice:hunter2@[::1".into()),
            ..TelemetryConfig::default()
        };
        let captured = capture_json(Level::INFO, || {
            assert!(init_logger_provider(&config).is_none());
            assert!(init_meter_provider(&config).is_none());
        });

        assert_eq!(captured.events.len(), 2, "{}", captured.raw);
        assert!(!captured.raw.contains("hunter2"), "{}", captured.raw);
        for event in &captured.events {
            assert_eq!(event["level"], "WARN");
            assert!(event["error.message"].is_string(), "{event}");
        }
    }

    #[test]
    fn resource_carries_service_identity_and_environment() {
        let resource = super::resource("tumultd", Some("prod"));
        let get = |key: &'static str| {
            resource
                .get(&Key::from_static_str(key))
                .map(|v| v.to_string())
        };
        assert_eq!(get("service.name").as_deref(), Some("tumultd"));
        assert_eq!(
            get("service.version").as_deref(),
            Some(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(get("deployment.environment.name").as_deref(), Some("prod"));
    }

    #[test]
    fn resource_without_an_environment_has_no_environment_attribute() {
        let resource = super::resource("tumultd", None);
        // `OTEL_RESOURCE_ATTRIBUTES` may legitimately set it in the test
        // environment; only assert when it does not.
        if std::env::var_os("OTEL_RESOURCE_ATTRIBUTES").is_none() {
            assert!(resource
                .get(&Key::from_static_str("deployment.environment.name"))
                .is_none());
        }
    }

    #[test]
    fn new_never_installs_a_global_subscriber() {
        // `shutdown` resets the global meter provider.
        let _meter_lock = crate::tests::METER_MUTEX.lock().unwrap();
        let telemetry = super::TumultTelemetry::new(TelemetryConfig {
            enabled: false,
            ..TelemetryConfig::default()
        });
        telemetry.shutdown();
        // Installing the subscriber is `logging::init`'s job alone; nothing
        // in this test binary calls it, so the global default must still be
        // the no-op subscriber.
        let global_is_noop = tracing::dispatcher::get_default(|dispatch| {
            dispatch.is::<tracing::subscriber::NoSubscriber>()
        });
        assert!(global_is_noop);
    }
}
