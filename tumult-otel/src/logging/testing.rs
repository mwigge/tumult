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

use serde_json::Value;
use tracing::subscriber::DefaultGuard;
use tracing::Level;
use tracing_subscriber::fmt::MakeWriter;

use super::{filter, EventFormatter, LogFormat, LogResource, Pipeline};

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

    use super::{capture_json, LogCapture, SERVICE_NAME};

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
    fn captures_do_not_leak_between_threads() {
        let capture = LogCapture::start(Level::INFO);
        std::thread::spawn(|| tracing::warn!(name: "a.other_thread", "elsewhere"))
            .join()
            .unwrap();
        assert!(capture.snapshot().events.is_empty());
    }
}
