//! The OTLP log bridge: mirrors `tracing` events to the collector.
//!
//! The subscriber is installed once, at process start, but the OTLP
//! exporter can only be built later (the daemon has to compute its own
//! loopback endpoint first). The bridge therefore sits in the subscriber
//! from the start as an empty slot and is filled in when the logger
//! provider exists.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use tracing::{span, Event, Subscriber};
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::{Context, Filter};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

use super::capped::{CappedLogger, CappedProvider};
use super::filter::is_exportable;

type Bridge = OpenTelemetryTracingBridge<CappedProvider, CappedLogger>;

/// Why a logger provider could not be attached to the log bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AttachError {
    /// A logger provider is already attached; the bridge exports to exactly
    /// one provider for the life of the process.
    #[error("a logger provider is already attached to the log bridge")]
    AlreadyAttached,
    /// [`init`](super::init) has not installed the subscriber, so there is
    /// no bridge to attach to.
    #[error("logging is not initialised; call tumult_otel::logging::init first")]
    NotInitialized,
}

/// The place in the subscriber where the OTLP log bridge goes once a logger
/// provider exists. Cloning shares the same slot.
#[derive(Clone, Default)]
pub(crate) struct LogBridgeSlot(Arc<SlotState>);

#[derive(Default)]
struct SlotState {
    bridge: OnceLock<Bridge>,
    /// Set by [`LogBridgeSlot::detach`]; the bridge is never used again.
    detached: AtomicBool,
}

impl LogBridgeSlot {
    /// Starts exporting events to `provider`.
    pub(crate) fn attach(&self, provider: &SdkLoggerProvider) -> Result<(), AttachError> {
        let capped = CappedProvider::new(provider.clone());
        self.0
            .bridge
            .set(OpenTelemetryTracingBridge::new(&capped))
            .map_err(|_| AttachError::AlreadyAttached)
    }

    /// Stops exporting, for good. Call it before shutting the provider down:
    /// the SDK logs a warning for every record emitted to a provider that is
    /// shut down, and that warning would follow every later event.
    pub(crate) fn detach(&self) {
        self.0.detached.store(true, Ordering::Release);
    }

    /// The bridge, while one is attached and not detached.
    fn bridge(&self) -> Option<&Bridge> {
        if self.0.detached.load(Ordering::Acquire) {
            return None;
        }
        self.0.bridge.get()
    }

    /// The subscriber layer for this slot, wrapped in the filter that keeps
    /// the exporter's own events out of the export.
    #[cfg(test)]
    pub(crate) fn layer<S>(&self) -> impl Layer<S>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        self.layer_keeping_local(Vec::new())
    }

    /// [`layer`](Self::layer), which also keeps the events of the
    /// `local_only` modules out of the export.
    pub(crate) fn layer_keeping_local<S>(&self, local_only: Vec<String>) -> impl Layer<S>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        LateBridge(self.clone()).with_filter(export_filter(local_only))
    }
}

/// Passes every event except those from the exporter's own stack and from
/// the `local_only` modules. It is a per-layer filter, so it holds whatever
/// `RUST_LOG` is set to.
fn export_filter<S>(local_only: Vec<String>) -> impl Filter<S> {
    filter_fn(move |metadata| is_exportable(metadata.target(), &local_only))
}

/// Forwards to the bridge while the slot holds one; a no-op before a
/// provider is attached and after it is detached.
struct LateBridge(LogBridgeSlot);

impl LateBridge {
    fn bridge(&self) -> Option<&Bridge> {
        self.0.bridge()
    }
}

impl<S> Layer<S> for LateBridge
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        if let Some(bridge) = self.bridge() {
            bridge.on_new_span(attrs, id, ctx);
        }
    }

    fn on_record(&self, id: &span::Id, values: &span::Record<'_>, ctx: Context<'_, S>) {
        if let Some(bridge) = self.bridge() {
            bridge.on_record(id, values, ctx);
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        if let Some(bridge) = self.bridge() {
            bridge.on_event(event, ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::trace::{TraceContextExt, Tracer, TracerProvider};
    use opentelemetry_sdk::logs::{InMemoryLogExporter, SdkLoggerProvider};
    use tracing_subscriber::layer::SubscriberExt;

    use super::{AttachError, LogBridgeSlot};
    use crate::logging::fields::{bounded, MAX_FIELD_BYTES};

    fn provider() -> (SdkLoggerProvider, InMemoryLogExporter) {
        let exporter = InMemoryLogExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        (provider, exporter)
    }

    /// The `(target, body)` of every exported record.
    fn exported(exporter: &InMemoryLogExporter) -> Vec<(String, String)> {
        exporter
            .get_emitted_logs()
            .unwrap()
            .into_iter()
            .map(|log| {
                let target = log.record.target().map(ToString::to_string);
                let body = match log.record.body() {
                    Some(opentelemetry::logs::AnyValue::String(body)) => body.to_string(),
                    other => format!("{other:?}"),
                };
                (target.unwrap_or_default(), body)
            })
            .collect()
    }

    #[test]
    fn events_before_attach_are_dropped_and_events_after_are_exported() {
        let slot = LogBridgeSlot::default();
        let (provider, exporter) = provider();
        let subscriber = tracing_subscriber::registry().with(slot.layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "tumultd", "before attach");
            slot.attach(&provider).unwrap();
            tracing::info!(target: "tumultd", "after attach");
        });
        assert_eq!(
            exported(&exporter),
            [("tumultd".to_string(), "after attach".to_string())]
        );
    }

    #[test]
    fn exporter_internal_targets_never_reach_the_exporter() {
        let slot = LogBridgeSlot::default();
        let (provider, exporter) = provider();
        slot.attach(&provider).unwrap();
        let subscriber = tracing_subscriber::registry().with(slot.layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!(target: "opentelemetry_sdk", "sdk export failed");
            tracing::error!(target: "opentelemetry-otlp", "otlp export failed");
            tracing::warn!(target: "tonic::transport::channel", "tonic");
            tracing::warn!(target: "hyper::client::conn", "hyper");
            tracing::warn!(target: "hyper_util::client", "hyper util");
            tracing::warn!(target: "h2::codec", "h2");
            tracing::warn!(target: "tower::buffer", "tower");
            tracing::warn!(target: "reqwest::connect", "reqwest");
            tracing::warn!(target: "tumult_api::auth::session", "login failed");
        });
        assert_eq!(
            exported(&exporter),
            [(
                "tumult_api::auth::session".to_string(),
                "login failed".to_string()
            )]
        );
    }

    #[test]
    fn local_only_modules_never_reach_the_exporter() {
        let slot = LogBridgeSlot::default();
        let (provider, exporter) = provider();
        slot.attach(&provider).unwrap();
        let local_only = vec!["tumult_ingest::writer".to_string()];
        let subscriber = tracing_subscriber::registry().with(slot.layer_keeping_local(local_only));
        tracing::subscriber::with_default(subscriber, || {
            tracing::error!(target: "tumult_ingest::writer", "failed to persist ingest batch");
            tracing::error!(target: "tumult_ingest::writer::retry", "reconnect failed");
            tracing::error!(target: "tumult_ingest::runs", "run failed");
        });
        assert_eq!(
            exported(&exporter),
            [("tumult_ingest::runs".to_string(), "run failed".to_string())]
        );
    }

    #[test]
    fn events_after_detach_are_not_exported() {
        let slot = LogBridgeSlot::default();
        let (provider, exporter) = provider();
        slot.attach(&provider).unwrap();
        let subscriber = tracing_subscriber::registry().with(slot.layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "tumultd", "before detach");
            slot.detach();
            tracing::info!(target: "tumultd", "after detach");
        });
        assert_eq!(
            exported(&exporter),
            [("tumultd".to_string(), "before detach".to_string())]
        );
    }

    #[test]
    fn a_detached_slot_takes_no_new_provider() {
        let slot = LogBridgeSlot::default();
        let (provider, _) = provider();
        slot.attach(&provider).unwrap();
        slot.detach();
        assert_eq!(slot.attach(&provider), Err(AttachError::AlreadyAttached));
    }

    #[test]
    fn a_second_attach_is_rejected() {
        let slot = LogBridgeSlot::default();
        let (provider, _) = provider();
        slot.attach(&provider).unwrap();
        assert_eq!(slot.attach(&provider), Err(AttachError::AlreadyAttached));
    }

    #[test]
    fn stable_event_name_becomes_the_otlp_event_name() {
        let slot = LogBridgeSlot::default();
        let (provider, exporter) = provider();
        slot.attach(&provider).unwrap();
        let subscriber = tracing_subscriber::registry().with(slot.layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(name: "auth.login.failed", { user.name = "alice" }, "login failed");
        });
        let logs = exporter.get_emitted_logs().unwrap();
        assert_eq!(logs[0].record.event_name(), Some("auth.login.failed"));
    }

    /// Byte lengths of every string the exporter received: the body, then
    /// each string attribute.
    fn exported_string_lengths(exporter: &InMemoryLogExporter) -> Vec<usize> {
        let logs = exporter.get_emitted_logs().unwrap();
        let record = &logs[0].record;
        record
            .body()
            .into_iter()
            .chain(record.attributes_iter().map(|(_, value)| value))
            .filter_map(|value| match value {
                opentelemetry::logs::AnyValue::String(text) => Some(text.as_str().len()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn exported_strings_are_capped_like_the_local_line() {
        let slot = LogBridgeSlot::default();
        let (provider, exporter) = provider();
        slot.attach(&provider).unwrap();
        let subscriber = tracing_subscriber::registry().with(slot.layer());
        let huge = "\u{e9}".repeat(MAX_FIELD_BYTES * 2);
        let capped = bounded(&huge).len();
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(
                name: "plugin.output",
                { process.stdout = huge.as_str(), process.debugged = ?huge, rows = 3 },
                "{huge}"
            );
        });
        let lengths = exported_string_lengths(&exporter);
        assert_eq!(lengths.len(), 3, "body and two string attributes");
        assert!(capped < huge.len());
        for length in lengths {
            assert!(length <= capped, "{length} bytes exported, cap {capped}");
        }
    }

    #[test]
    fn short_exported_strings_are_left_alone() {
        let slot = LogBridgeSlot::default();
        let (provider, exporter) = provider();
        slot.attach(&provider).unwrap();
        let subscriber = tracing_subscriber::registry().with(slot.layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(name: "auth.login.failed", { user.name = "alice" }, "login failed");
        });
        assert_eq!(
            exported_string_lengths(&exporter),
            ["login failed".len(), "alice".len()]
        );
    }

    #[test]
    fn exported_record_carries_the_active_trace_context() {
        let slot = LogBridgeSlot::default();
        let (provider, exporter) = provider();
        slot.attach(&provider).unwrap();
        let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let tracer = tracer_provider.tracer("bridge-tests");
        let subscriber = tracing_subscriber::registry().with(slot.layer());
        let trace_id = tracing::subscriber::with_default(subscriber, || {
            let cx = opentelemetry::Context::current_with_span(tracer.start("request"));
            let trace_id = cx.span().span_context().trace_id();
            let _guard = cx.attach();
            tracing::info!(target: "tumultd", "inside");
            trace_id
        });
        let logs = exporter.get_emitted_logs().unwrap();
        let context = logs[0].record.trace_context().expect("trace context");
        assert_eq!(context.trace_id, trace_id);
    }
}
