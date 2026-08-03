use super::*;

// ── Panic-safe env-var guard (ERR-04, TEST-04) ─────────────────────────
//
// `std::env::set_var` is not thread-safe and is deprecated as `unsafe` in
// Rust 1.80+.  We serialise all env-var mutations via `ENV_MUTEX` (already
// present) and additionally wrap each mutation in this `Drop` guard so that
// if the assertion between `set_var` and the restoration panics, the env var
// is still restored before the next test acquires the mutex.

static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Serialises tests that touch the GLOBAL meter provider.
///
/// `TumultTelemetry::new` registers a global meter provider when an OTLP
/// endpoint is set, and `shutdown()` resets it to noop — while the
/// production-path smoke test installs its own in-memory provider. Without
/// this lock, parallel test threads could interleave a noop reset between
/// another test's `set_meter_provider` and its recording, making it flaky.
static METER_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard that restores an environment variable to its previous value
/// when dropped, even on panic.
struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    /// Save the current value of `key` and set it to `value`.
    ///
    /// The caller must hold `ENV_MUTEX` for the entire lifetime of the guard.
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: we hold ENV_MUTEX, so no other thread can concurrently
        // read or write this env var via this crate's test suite.
        #[allow(unused_unsafe)] // safe in single-threaded test context with mutex held
        unsafe {
            std::env::set_var(key, value);
        }
        Self { key, prev }
    }

    /// Remove `key` from the environment and save its current value.
    fn remove(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        // SAFETY: we hold ENV_MUTEX (see `set` above).
        #[allow(unused_unsafe)]
        unsafe {
            std::env::remove_var(key);
        }
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: called while ENV_MUTEX is still held by the test that
        // created this guard (guards are dropped at end of scope, before
        // the MutexGuard is dropped).
        #[allow(unused_unsafe)]
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

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
    let _lock = ENV_MUTEX.lock().unwrap();
    let _guard = EnvGuard::set("TUMULT_OTEL_ENABLED", "false");
    let config = TelemetryConfig::from_env();
    assert!(!config.enabled);
    // _guard restores the env var on drop (even if the assertion above panics).
}

#[test]
fn config_from_env_defaults_to_enabled() {
    let _lock = ENV_MUTEX.lock().unwrap();
    let _guard = EnvGuard::remove("TUMULT_OTEL_ENABLED");
    let config = TelemetryConfig::from_env();
    assert!(config.enabled);
    // _guard restores the env var on drop.
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
    // Records through the global meter: hold the lock so these records
    // cannot interleave with a provider-installing test (which asserts
    // exact counter totals) — the source of an observed parallel flake.
    let _meter_lock = METER_MUTEX.lock().unwrap();

    let meter = opentelemetry::global::meter("integration-test");
    let metrics = TumultMetrics::new(&meter);

    // Simulate a full experiment sequence
    instrument::record_experiment(&metrics, true);

    let start = std::time::Instant::now();
    instrument::record_action(&metrics, "tumult-db", "kill-connections", start, true);
    instrument::record_probe(&metrics, "tumult-http", "health-check", start, true);
    instrument::record_probe(&metrics, "tumult-http", "health-check", start, false);
    instrument::record_deviation(&metrics, "integration-test-experiment");
    instrument::record_action(&metrics, "tumult-db", "restore-pool", start, true);
    instrument::record_experiment(&metrics, false);
}

// ── TumultTelemetry ────────────────────────────────────────

#[test]
fn telemetry_disabled_config_does_not_initialize_providers() {
    let _meter_lock = METER_MUTEX.lock().unwrap();
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
    let _meter_lock = METER_MUTEX.lock().unwrap();
    let config = TelemetryConfig {
        enabled: true,
        otlp_endpoint: None,
        ..TelemetryConfig::default()
    };
    let telemetry = TumultTelemetry::new(config);
    // No OTLP endpoint and no console export means no provider is built,
    // so is_enabled() correctly returns false — telemetry is configured but
    // no spans are exported.  This is the correct behaviour after ERR-06 fix.
    assert!(!telemetry.is_enabled());
    assert_eq!(telemetry.service_name(), "tumult");
    telemetry.shutdown();
}

#[test]
fn telemetry_enabled_with_endpoint_initializes() {
    let _meter_lock = METER_MUTEX.lock().unwrap();
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

#[test]
fn telemetry_console_export_without_endpoint_does_not_panic() {
    let _meter_lock = METER_MUTEX.lock().unwrap();
    let config = TelemetryConfig {
        enabled: true,
        console_export: true,
        otlp_endpoint: None,
        ..TelemetryConfig::default()
    };
    let telemetry = TumultTelemetry::new(config);
    assert!(telemetry.is_enabled());
    telemetry.shutdown();
}

/// Verifies that `shutdown()` also shuts down the globally registered
/// `TracerProvider` so that spans emitted after shutdown are silently
/// dropped rather than sent to an already-closed exporter.
///
/// After `global::shutdown_tracer_provider()` the global provider
/// transitions to a noop state.  The observable contract here is that
/// `shutdown()` must not panic even when the global provider was set
/// during `new()`.
#[test]
fn shutdown_also_shuts_down_global_provider_without_panic() {
    use opentelemetry::global;

    let _meter_lock = METER_MUTEX.lock().unwrap();

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

    // Calling shutdown() must not panic, and after it returns the global
    // provider must not hold any live exporters.  We verify by obtaining
    // a tracer from the global — this should succeed (noop, no panic).
    telemetry.shutdown();

    // Attempting to get a tracer from the global must not panic.
    let _tracer = global::tracer("post-shutdown-tracer");
}

// ── init_meter_provider ────────────────────────────────────

#[test]
fn init_meter_provider_returns_none_when_disabled() {
    let config = TelemetryConfig {
        enabled: false,
        otlp_endpoint: Some("http://localhost:4317".into()),
        ..TelemetryConfig::default()
    };
    assert!(telemetry::init_meter_provider(&config).is_none());
}

#[test]
fn init_meter_provider_returns_none_without_endpoint() {
    let config = TelemetryConfig {
        enabled: true,
        otlp_endpoint: None,
        ..TelemetryConfig::default()
    };
    assert!(telemetry::init_meter_provider(&config).is_none());
}

#[test]
fn init_meter_provider_builds_provider_with_endpoint() {
    let _meter_lock = METER_MUTEX.lock().unwrap();
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
    let provider = telemetry::init_meter_provider(&config);
    assert!(provider.is_some());
    // Clean up: shut down and reset the global so later tests start noop.
    if let Some(p) = provider {
        let _ = p.shutdown();
    }
    opentelemetry::global::set_meter_provider(
        opentelemetry::metrics::noop::NoopMeterProvider::new(),
    );
}

/// Production-path smoke test: with a real `SdkMeterProvider` registered
/// globally, instruments obtained via `global::meter(...)` — the exact
/// path every production caller takes — must actually record. Before the
/// meter-provider init existed, the global meter stayed noop and every
/// counter silently vanished.
#[test]
fn global_meter_records_counter_with_real_provider() {
    use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, SumDataPoint};
    use opentelemetry_sdk::metrics::{InMemoryMetricExporter, SdkMeterProvider};

    let _meter_lock = METER_MUTEX.lock().unwrap();

    let exporter = InMemoryMetricExporter::default();
    let provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter.clone())
        .build();
    opentelemetry::global::set_meter_provider(provider.clone());

    // The production path: global meter -> TumultMetrics -> record.
    let meter = opentelemetry::global::meter("smoke-test");
    let metrics = TumultMetrics::new(&meter);
    instrument::record_experiment(&metrics, true);
    instrument::record_experiment(&metrics, false);

    provider.force_flush().unwrap();

    let mut recorded_total = None;
    for rm in exporter.get_finished_metrics().unwrap_or_default() {
        for sm in rm.scope_metrics() {
            for metric in sm.metrics() {
                if metric.name() == "tumult.experiments.total" {
                    if let AggregatedMetrics::U64(MetricData::Sum(sum)) = metric.data() {
                        recorded_total =
                            Some(sum.data_points().map(SumDataPoint::value).sum::<u64>());
                    }
                }
            }
        }
    }
    assert_eq!(
        recorded_total,
        Some(2),
        "counter recorded through the global meter must reach the exporter"
    );

    // Clean up: shut down and reset the global so later tests start noop.
    let _ = provider.shutdown();
    opentelemetry::global::set_meter_provider(
        opentelemetry::metrics::noop::NoopMeterProvider::new(),
    );
}

#[test]
fn init_meter_provider_builds_provider_with_explicit_headers() {
    let _meter_lock = METER_MUTEX.lock().unwrap();
    let rt = tokio_minimal::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _guard = rt.enter();
    let mut headers = tonic::metadata::MetadataMap::new();
    headers.insert("authorization", "Bearer kro_cfg".parse().unwrap());
    let config = TelemetryConfig {
        enabled: true,
        otlp_endpoint: Some("http://localhost:4317".into()),
        otlp_headers: Some(headers),
        ..TelemetryConfig::default()
    };
    let provider = telemetry::init_meter_provider(&config);
    assert!(provider.is_some());
    // Clean up: shut down and reset the global so later tests start noop.
    if let Some(p) = provider {
        let _ = p.shutdown();
    }
    opentelemetry::global::set_meter_provider(
        opentelemetry::metrics::noop::NoopMeterProvider::new(),
    );
}

#[test]
fn telemetry_with_endpoint_and_explicit_headers_initializes() {
    let _meter_lock = METER_MUTEX.lock().unwrap();
    let rt = tokio_minimal::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let _guard = rt.enter();
    let mut headers = tonic::metadata::MetadataMap::new();
    headers.insert("authorization", "Bearer kro_cfg".parse().unwrap());
    let config = TelemetryConfig {
        enabled: true,
        otlp_endpoint: Some("http://localhost:4317".into()),
        otlp_headers: Some(headers),
        ..TelemetryConfig::default()
    };
    let telemetry = TumultTelemetry::new(config);
    assert!(telemetry.is_enabled());
    telemetry.shutdown();
}
