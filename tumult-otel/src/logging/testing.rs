//! Capturing log output in tests.
//!
//! This module is compiled into the library rather than behind
//! `cfg(test)` so that every crate's tests can assert on the events that
//! crate emits — which fields are present, and above all which values are
//! absent. Events go through the same filter and JSON formatter as in
//! production, but into memory instead of stderr.
//!
//! ```
//! use tracing::Level;
//! use tumult_otel::logging::testing::capture_json;
//!
//! let logs = capture_json(Level::INFO, || {
//!     tracing::warn!(name: "auth.login.failed", { user.name = "alice" }, "login failed");
//! });
//! assert_eq!(logs.events[0]["event.name"], "auth.login.failed");
//! assert_eq!(logs.events[0]["user.name"], "alice");
//! ```

use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use opentelemetry::logs::AnyValue;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::logs::{LogBatch, LogExporter, SdkLoggerProvider};
use serde_json::Value;
use tracing::subscriber::DefaultGuard;
use tracing::Level;
use tracing_subscriber::fmt::MakeWriter;

use super::{filter, EventFormatter, LogFormat, LogOptions, LogResource, Pipeline};

/// The `service.name` on captured lines.
pub const SERVICE_NAME: &str = "tumult-test";

/// An in-memory log sink shared between a subscriber and a test.
#[derive(Clone, Debug, Default)]
pub struct Sink(Arc<Mutex<Vec<u8>>>);

impl Sink {
    /// Everything written so far, as text.
    #[must_use]
    pub fn text(&self) -> String {
        let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl io::Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Sink {
    type Writer = Sink;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// What a closure logged; see [`capture_json`].
#[derive(Debug, Clone)]
pub struct Captured {
    /// The raw output: one JSON object per line.
    pub raw: String,
    /// Each line, parsed.
    pub events: Vec<Value>,
}

impl Captured {
    /// The events whose `event.name` is `name`.
    #[must_use]
    pub fn named(&self, name: &str) -> Vec<&Value> {
        self.events
            .iter()
            .filter(|event| event["event.name"] == name)
            .collect()
    }
}

/// Captures the current thread's events until dropped.
///
/// Use this instead of [`capture_json`] in async tests on a current-thread
/// runtime (the `#[tokio::test]` default), where the code under test cannot
/// be wrapped in a closure. Events from other threads are not captured.
pub struct LogCapture {
    sink: Sink,
    _guard: DefaultGuard,
}

impl LogCapture {
    /// Starts capturing events at `level` and above.
    #[must_use]
    pub fn start(level: Level) -> Self {
        let sink = Sink::default();
        let (subscriber, _slot) = pipeline(&sink, level).build();
        let guard = tracing::subscriber::set_default(subscriber);
        Self {
            sink,
            _guard: guard,
        }
    }

    /// Everything captured so far.
    ///
    /// # Panics
    ///
    /// When a captured line is not a JSON object — which would be a bug in
    /// the formatter, and should fail the test.
    #[must_use]
    pub fn snapshot(&self) -> Captured {
        parse(self.sink.text())
    }

    /// The sink the captured lines are written to. Unlike the capture it
    /// can be sent to another thread, to check there what has been logged
    /// so far.
    #[must_use]
    pub fn sink(&self) -> Sink {
        self.sink.clone()
    }
}

/// A log record as the OTLP exporter received it; see [`ExportCapture`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedEvent {
    /// The module the event came from.
    pub target: String,
    /// The event's name, when it has one.
    pub event_name: Option<String>,
    /// The event's message, when it is text.
    pub body: Option<String>,
}

/// Captures the current thread's events twice until dropped: the lines
/// written locally, and the records handed to the OTLP exporter.
///
/// The subscriber is built from a binary's own [`LogOptions`], so a test
/// can assert on what that binary would export — above all on what it must
/// not. Records reach the exporter as they are emitted; there is no batch
/// to wait for.
pub struct ExportCapture {
    sink: Sink,
    exporter: RecordingExporter,
    // Declared before the provider: the subscriber goes first, so nothing
    // is emitted into a provider that is shutting down.
    _guard: DefaultGuard,
    _provider: SdkLoggerProvider,
}

impl ExportCapture {
    /// Starts capturing with the level and local-only targets of `options`.
    ///
    /// # Panics
    ///
    /// When the freshly built subscriber already has an exporter attached,
    /// which cannot happen.
    #[must_use]
    pub fn start(options: &LogOptions) -> Self {
        let sink = Sink::default();
        let exporter = RecordingExporter::default();
        let provider = SdkLoggerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        let pipeline = Pipeline {
            local_only: options.local_only_targets.clone(),
            ..pipeline(&sink, options.default_level)
        };
        let (subscriber, slot) = pipeline.build();
        slot.attach(&provider)
            .expect("a new bridge slot has nothing attached");
        Self {
            sink,
            exporter,
            _guard: tracing::subscriber::set_default(subscriber),
            _provider: provider,
        }
    }

    /// Every record exported so far, oldest first.
    #[must_use]
    pub fn exported(&self) -> Vec<ExportedEvent> {
        self.exporter.events()
    }

    /// Everything written locally so far.
    ///
    /// # Panics
    ///
    /// When a captured line is not a JSON object — which would be a bug in
    /// the formatter, and should fail the test.
    #[must_use]
    pub fn local(&self) -> Captured {
        parse(self.sink.text())
    }
}

/// An exporter that keeps what it is given.
#[derive(Debug, Clone, Default)]
struct RecordingExporter(Arc<Mutex<Vec<ExportedEvent>>>);

impl RecordingExporter {
    fn events(&self) -> Vec<ExportedEvent> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl LogExporter for RecordingExporter {
    fn export(
        &self,
        batch: LogBatch<'_>,
    ) -> impl std::future::Future<Output = OTelSdkResult> + Send {
        let mut events = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        for (record, _scope) in batch.iter() {
            events.push(ExportedEvent {
                target: record.target().map(ToString::to_string).unwrap_or_default(),
                event_name: record.event_name().map(str::to_string),
                body: match record.body() {
                    Some(AnyValue::String(text)) => Some(text.as_str().to_string()),
                    _ => None,
                },
            });
        }
        std::future::ready(Ok(()))
    }
}

/// Runs `emit` and returns what it logged at `level` and above.
///
/// # Panics
///
/// When a captured line is not a JSON object — which would be a bug in the
/// formatter, and should fail the test.
pub fn capture_json(level: Level, emit: impl FnOnce()) -> Captured {
    let sink = Sink::default();
    let (subscriber, _slot) = pipeline(&sink, level).build();
    tracing::subscriber::with_default(subscriber, emit);
    parse(sink.text())
}

fn pipeline(sink: &Sink, level: Level) -> Pipeline<Sink> {
    let resource = LogResource {
        service_name: SERVICE_NAME.to_string(),
        service_version: super::SERVICE_VERSION.to_string(),
        environment: None,
    };
    Pipeline {
        formatter: EventFormatter::new(LogFormat::Json, resource),
        filter: filter::env_filter(None, level).0,
        writer: sink.clone(),
        ansi: false,
        local_only: Vec::new(),
        output_limit: None,
    }
}

fn parse(raw: String) -> Captured {
    let events = raw
        .lines()
        .map(|line| match serde_json::from_str::<Value>(line) {
            Ok(event @ Value::Object(_)) => event,
            other => panic!("log line is not a JSON object ({other:?}): {line}"),
        })
        .collect();
    Captured { raw, events }
}

#[cfg(test)]
mod tests {
    use tracing::Level;

    use super::{capture_json, ExportCapture, ExportedEvent, LogCapture, SERVICE_NAME};
    use crate::logging::LogOptions;

    #[test]
    fn export_capture_separates_what_is_logged_from_what_is_exported() {
        let options = LogOptions::new("tumultd").with_local_only_targets(["tumult_ingest"]);
        let capture = ExportCapture::start(&options);
        tracing::error!(target: "tumult_ingest::writer", "failed to persist ingest batch");
        tracing::warn!(name: "daemon.stopped", "daemon stopped");
        tracing::debug!(name: "a.hidden", "below the level");

        assert_eq!(capture.local().events.len(), 2);
        assert_eq!(
            capture.exported(),
            [ExportedEvent {
                target: module_path!().to_string(),
                event_name: Some("daemon.stopped".to_string()),
                body: Some("daemon stopped".to_string()),
            }]
        );
    }

    #[test]
    fn closure_capture_returns_parsed_events_in_order() {
        let logs = capture_json(Level::INFO, || {
            tracing::info!(name: "a.first", { rows = 3 }, "first");
            tracing::debug!(name: "a.hidden", "below the level");
            tracing::warn!(name: "a.second", "second");
        });
        assert_eq!(logs.events.len(), 2);
        assert_eq!(logs.events[0]["rows"], 3);
        assert_eq!(logs.events[0]["service.name"], SERVICE_NAME);
        assert_eq!(logs.named("a.second").len(), 1);
        assert!(logs.named("a.hidden").is_empty());
        assert_eq!(logs.raw.lines().count(), 2);
    }

    #[test]
    fn guard_capture_sees_events_until_it_is_dropped() {
        let capture = LogCapture::start(Level::WARN);
        tracing::info!(name: "a.info", "filtered out");
        tracing::warn!(name: "a.warn", "kept");
        let logs = capture.snapshot();
        assert_eq!(logs.events.len(), 1);
        assert_eq!(logs.events[0]["event.name"], "a.warn");
        drop(capture);
        tracing::warn!(name: "a.after", "nobody is listening");
    }

    #[test]
    fn the_sink_of_a_capture_can_be_read_from_another_thread() {
        let capture = LogCapture::start(Level::INFO);
        let sink = capture.sink();
        tracing::warn!(name: "a.warn", "kept");
        let seen = std::thread::spawn(move || sink.text()).join().unwrap();
        assert!(seen.contains("\"a.warn\""), "{seen}");
        assert_eq!(seen, capture.snapshot().raw);
    }

    #[test]
    fn captures_do_not_leak_between_threads() {
        let capture = LogCapture::start(Level::INFO);
        std::thread::spawn(|| tracing::warn!(name: "a.other_thread", "elsewhere"))
            .join()
            .unwrap();
        assert!(capture.snapshot().events.is_empty());
    }
}
