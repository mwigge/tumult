//! The OTLP log bridge: mirrors `tracing` events to the collector.
//!
//! The subscriber is installed once, at process start, but the OTLP
//! exporter can only be built later (the daemon has to compute its own
//! loopback endpoint first). The bridge therefore sits in the subscriber
//! from the start as an empty slot and is filled in when the logger
//! provider exists.

use std::sync::{Arc, OnceLock};

use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_sdk::logs::{SdkLogger, SdkLoggerProvider};
use tracing::{span, Event, Subscriber};
use tracing_subscriber::filter::{filter_fn, FilterExt};
use tracing_subscriber::layer::{Context, Filter};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

use super::filter::is_exporter_internal;

type Bridge = OpenTelemetryTracingBridge<SdkLoggerProvider, SdkLogger>;

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
pub(crate) struct LogBridgeSlot(Arc<OnceLock<Bridge>>);

impl LogBridgeSlot {
    /// Starts exporting events to `provider`.
    pub(crate) fn attach(&self, provider: &SdkLoggerProvider) -> Result<(), AttachError> {
        self.0
            .set(OpenTelemetryTracingBridge::new(provider))
            .map_err(|_| AttachError::AlreadyAttached)
    }

    /// The subscriber layer for this slot, wrapped in the filter that keeps
    /// the exporter's own events out of the export.
    pub(crate) fn layer<S>(&self) -> impl Layer<S>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        LateBridge(self.clone()).with_filter(export_filter())
    }
}

/// Passes every event except those from the exporter's own stack. It is a
/// per-layer filter, so it holds whatever `RUST_LOG` is set to.
fn export_filter<S>() -> impl Filter<S> {
    filter_fn(|metadata| is_exporter_internal(metadata.target())).not()
}

/// Forwards to the bridge once the slot is filled; a no-op before that.
struct LateBridge(LogBridgeSlot);

impl LateBridge {
    fn bridge(&self) -> Option<&Bridge> {
        (self.0).0.get()
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
