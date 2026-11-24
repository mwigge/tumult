use super::*;
use tumult_test_utils::EnvGuard;

/// Serialises tests that touch the GLOBAL meter provider.
///
/// `TumultTelemetry::new` registers a global meter provider when an OTLP
/// endpoint is set, and `shutdown()` resets it to noop — while the
/// production-path smoke test installs its own in-memory provider. Without
/// this lock, parallel test threads could interleave a noop reset between
/// another test's `set_meter_provider` and its recording, making it flaky.
pub(crate) static METER_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    let mut env = EnvGuard::lock();
    env.set("TUMULT_OTEL_ENABLED", "false");
    let config = TelemetryConfig::from_env();
    assert!(!config.enabled);
}

#[test]
fn config_from_env_defaults_to_enabled() {
    let mut env = EnvGuard::lock();
    env.remove("TUMULT_OTEL_ENABLED");
    let config = TelemetryConfig::from_env();
    assert!(config.enabled);
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
        attributes::IMPACT_OBSERVED,
        attributes::RECOVERED,
        attributes::RTO_TARGET_S,
        attributes::RTO_STATE,
        attributes::RPO_TARGET_S,
        attributes::RPO_MEASURED_S,
        attributes::RPO_STATE,
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
