//! The JSON and text event formatters.
//!
//! Both render the same captured [`EventRecord`]; they differ only in how
//! the record is written. JSON is one object per line, serialised with
//! `serde_json` so every control character in a value is escaped. Text is a
//! single human-readable line for terminals.

use std::borrow::Cow;
use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};
use opentelemetry::trace::TraceContextExt;
use serde::ser::{Serialize, Serializer};
use serde_json::{Map, Value};
use tracing::{Event, Level, Subscriber};
use tracing_log::NormalizeEvent;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormattedFields, MakeWriter};
use tracing_subscriber::registry::LookupSpan;

use super::fields::{bounded, FieldCollector, TypedFields};

/// Output format of the local log stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// One JSON object per line. The default, and what every non-terminal
    /// writer gets.
    Json,
    /// One human-readable line per event, for terminals.
    Text,
}

/// Process-wide attributes stamped on every JSON log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogResource {
    /// Emitted as `service.name`.
    pub service_name: String,
    /// Emitted as `service.version`.
    pub service_version: String,
    /// Emitted as `deployment.environment.name` when known.
    pub environment: Option<String>,
}

/// Keys the formatter owns. An event field with one of these names is
/// emitted as `field.<name>` so it can never overwrite the real value.
const RESERVED_KEYS: [&str; 11] = [
    "timestamp",
    "level",
    "target",
    "event.name",
    "message",
    "service.name",
    "service.version",
    "deployment.environment.name",
    "trace_id",
    "span_id",
    "spans",
];

/// Prefix of the name `tracing` generates for events without `name:`
/// (`"event <file>:<line>"`). Such names change with every edit, so they
/// are not emitted.
const GENERATED_NAME_PREFIX: &str = "event ";

/// Prefix of the bookkeeping fields `tracing-log` adds to `log` records.
const LOG_BRIDGE_FIELD_PREFIX: &str = "log.";

/// Formats events as JSON or text, adding the timestamp, the service
/// identity and the active OpenTelemetry trace context.
#[derive(Debug, Clone)]
pub struct EventFormatter {
    format: LogFormat,
    resource: LogResource,
    clock: fn() -> DateTime<Utc>,
}

impl EventFormatter {
    /// A formatter for `format` that stamps `resource` on every line.
    #[must_use]
    pub fn new(format: LogFormat, resource: LogResource) -> Self {
        Self {
            format,
            resource,
            clock: Utc::now,
        }
    }

    /// Replaces the wall clock, so tests get deterministic timestamps.
    #[must_use]
    pub fn with_clock(mut self, clock: fn() -> DateTime<Utc>) -> Self {
        self.clock = clock;
        self
    }

    /// The line that ends a log cut off at `limit_bytes`; see
    /// [`LogOptions::with_output_limit`](super::LogOptions::with_output_limit).
    /// It is a JSON line in either format: what reads a capped log is a
    /// program.
    pub(crate) fn limit_marker(&self, limit_bytes: u64) -> String {
        let event = DirectEvent {
            name: LIMIT_EVENT,
            message: LIMIT_MESSAGE,
            fields: vec![("logging.output.limit_bytes", Value::from(limit_bytes))],
        };
        direct_line(
            &self.resource,
            (self.clock)(),
            module_path!(),
            Level::WARN,
            event,
        )
    }
}

const LIMIT_EVENT: &str = "logging.output.limit_reached";
const LIMIT_MESSAGE: &str = "log output limit reached; later events are dropped";

impl<S> FormatEvent<S, TypedFields> for EventFormatter
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, TypedFields>,
        writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let timestamp = (self.clock)().to_rfc3339_opts(SecondsFormat::Micros, true);
        let record = EventRecord::capture(timestamp, ctx, event);
        match self.format {
            LogFormat::Json => record.write_json(&self.resource, writer),
            LogFormat::Text => record.write_text(writer),
        }
    }
}

/// Builds the `fmt` layer that renders events with `formatter` into
/// `writer`. `ansi` only affects the text format.
pub(crate) fn event_layer<S, W>(
    formatter: EventFormatter,
    writer: W,
    ansi: bool,
) -> tracing_subscriber::fmt::Layer<S, TypedFields, EventFormatter, W>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    W: for<'writer> MakeWriter<'writer> + 'static,
{
    tracing_subscriber::fmt::layer()
        .fmt_fields(TypedFields)
        .event_format(formatter)
        .with_writer(writer)
        .with_ansi(ansi)
        // A failed write must stay silent. With this on, the layer reports
        // the failure with `eprintln!`, which panics when stderr is the
        // thing that failed (a closed pipe); the panic hook logs, fails the
        // same way, and the process aborts before any rollback runs.
        .log_internal_errors(false)
}

/// The identifiers of the OpenTelemetry span active when an event fired.
struct TraceIds {
    trace_id: String,
    span_id: String,
}

impl TraceIds {
    fn current() -> Option<Self> {
        let cx = opentelemetry::Context::current();
        let span = cx.span();
        let span_context = span.span_context();
        if !span_context.is_valid() {
            return None;
        }
        Some(Self {
            trace_id: span_context.trace_id().to_string(),
            span_id: span_context.span_id().to_string(),
        })
    }
}

/// One enclosing `tracing` span: its name and typed fields.
struct SpanRecord {
    name: &'static str,
    fields: Map<String, Value>,
}

impl SpanRecord {
    fn to_json(&self) -> Value {
        let mut line = JsonLine::default();
        line.reserved("name", Value::from(self.name));
        for (key, value) in &self.fields {
            let key = if key == "name" { "field.name" } else { key };
            line.field(key.to_string(), value.clone());
        }
        Value::Object(line.entries.into_iter().collect())
    }
}

/// Everything one log line is rendered from.
struct EventRecord {
    timestamp: String,
    level: Level,
    target: String,
    name: Option<&'static str>,
    message: String,
    fields: Vec<(String, Value)>,
    trace: Option<TraceIds>,
    spans: Vec<SpanRecord>,
}

impl EventRecord {
    fn capture<S>(
        timestamp: String,
        ctx: &FmtContext<'_, S, TypedFields>,
        event: &Event<'_>,
    ) -> Self
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        let mut collector = FieldCollector::default();
        event.record(&mut collector);
        // Records bridged from the `log` crate carry their real target in a
        // field; `normalized_metadata` recovers it.
        let from_log_crate = event.is_log();
        let normalized = event.normalized_metadata();
        let metadata = normalized.as_ref().unwrap_or_else(|| event.metadata());
        if from_log_crate {
            collector
                .fields
                .retain(|(key, _)| !key.starts_with(LOG_BRIDGE_FIELD_PREFIX));
        }
        let name = explicit_name(event, from_log_crate);
        Self {
            timestamp,
            level: *metadata.level(),
            target: metadata.target().to_string(),
            name,
            message: collector
                .message
                .or_else(|| name.map(str::to_string))
                .unwrap_or_default(),
            fields: collector.fields,
            trace: TraceIds::current(),
            spans: span_records(ctx),
        }
    }

    fn write_json(self, resource: &LogResource, mut writer: Writer<'_>) -> fmt::Result {
        let head = LineHead {
            timestamp: self.timestamp,
            level: self.level,
            target: self.target,
            name: self.name,
            message: self.message,
        };
        let mut line = JsonLine::start(head, resource);
        if let Some(trace) = self.trace {
            line.reserved("trace_id", Value::String(trace.trace_id));
            line.reserved("span_id", Value::String(trace.span_id));
        }
        if !self.spans.is_empty() {
            let spans = self.spans.iter().map(SpanRecord::to_json).collect();
            line.reserved("spans", Value::Array(spans));
        }
        for (key, value) in self.fields {
            line.field(key, value);
        }
        let json = line.render().map_err(|_| fmt::Error)?;
        writeln!(writer, "{json}")
    }

    fn write_text(&self, mut writer: Writer<'_>) -> fmt::Result {
        write!(writer, "{} ", self.timestamp)?;
        write_level(&mut writer, self.level)?;
        write!(writer, " {}: ", escape_text(&self.target))?;
        for span in &self.spans {
            write_text_span(&mut writer, span)?;
        }
        write!(writer, "{}", escape_text(&self.message))?;
        if let Some(name) = self.name {
            write!(writer, " event.name={name}")?;
        }
        for (key, value) in &self.fields {
            write!(writer, " {key}={}", json_text(value)?)?;
        }
        if let Some(trace) = &self.trace {
            write!(
                writer,
                " trace_id={} span_id={}",
                trace.trace_id, trace.span_id
            )?;
        }
        writeln!(writer)
    }
}

/// The event's stable name, when the call site set one with `name:`.
fn explicit_name(event: &Event<'_>, from_log_crate: bool) -> Option<&'static str> {
    if from_log_crate {
        return None;
    }
    let name = event.metadata().name();
    (!name.starts_with(GENERATED_NAME_PREFIX)).then_some(name)
}

/// The `tracing` spans enclosing the current event, root first.
fn span_records<S>(ctx: &FmtContext<'_, S, TypedFields>) -> Vec<SpanRecord>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    let Some(scope) = ctx.event_scope() else {
        return Vec::new();
    };
    scope
        .from_root()
        .map(|span| {
            let extensions = span.extensions();
            // The stored text was written by `TypedFields`, so it is always
            // a JSON object; a span whose fields were never formatted simply
            // has none to show.
            let fields = extensions
                .get::<FormattedFields<TypedFields>>()
                .and_then(|stored| serde_json::from_str(&stored.fields).ok())
                .unwrap_or_default();
            SpanRecord {
                name: span.name(),
                fields,
            }
        })
        .collect()
}

/// An error event written without going through the subscriber; see
/// [`error_line`].
pub(crate) struct DirectEvent {
    /// Emitted as `event.name`.
    pub(crate) name: &'static str,
    /// Emitted as `message`.
    pub(crate) message: &'static str,
    /// The event's own fields, in order.
    pub(crate) fields: Vec<(&'static str, Value)>,
}

/// Renders `event` as one ERROR line of the JSON format, for the moments
/// when the subscriber cannot be used: before it is installed, or when the
/// filter would drop an event that must not be lost.
pub(crate) fn error_line(
    resource: &LogResource,
    now: DateTime<Utc>,
    target: &str,
    event: DirectEvent,
) -> String {
    direct_line(resource, now, target, Level::ERROR, event)
}

/// Renders `event` as one line of the JSON format at `level`.
fn direct_line(
    resource: &LogResource,
    now: DateTime<Utc>,
    target: &str,
    level: Level,
    event: DirectEvent,
) -> String {
    let head = LineHead {
        timestamp: now.to_rfc3339_opts(SecondsFormat::Micros, true),
        level,
        target: target.to_string(),
        name: Some(event.name),
        message: event.message.to_string(),
    };
    let mut line = JsonLine::start(head, resource);
    for (key, value) in event.fields {
        line.field(key.to_string(), value);
    }
    // A map of JSON values always serialises; the fallback only keeps this
    // function total.
    line.render().unwrap_or_else(|_| {
        format!(
            "{{\"level\":\"{level}\",\"event.name\":\"{}\"}}",
            event.name
        )
    })
}

/// What every line starts with, whatever produced it.
struct LineHead {
    timestamp: String,
    level: Level,
    target: String,
    name: Option<&'static str>,
    message: String,
}

/// A JSON object whose keys keep insertion order, independent of how
/// `serde_json` was compiled.
#[derive(Default)]
struct JsonLine {
    entries: Vec<(String, Value)>,
}

impl JsonLine {
    /// A line holding the keys every line starts with, in their fixed
    /// order: `head`, then the service identity from `resource`.
    fn start(head: LineHead, resource: &LogResource) -> Self {
        let mut line = Self::default();
        line.reserved("timestamp", Value::String(head.timestamp));
        line.reserved("level", Value::from(head.level.as_str()));
        line.reserved("target", Value::String(head.target));
        if let Some(name) = head.name {
            line.reserved("event.name", Value::from(name));
        }
        line.reserved("message", Value::String(head.message));
        line.reserved("service.name", Value::from(resource.service_name.as_str()));
        line.reserved(
            "service.version",
            Value::from(resource.service_version.as_str()),
        );
        if let Some(environment) = &resource.environment {
            line.reserved(
                "deployment.environment.name",
                Value::from(bounded(environment).as_ref()),
            );
        }
        line
    }

    /// The line as text, every control character escaped.
    fn render(&self) -> Result<String, serde_json::Error> {
        let json = serde_json::to_string(self)?;
        Ok(escape_remaining_controls(&json).into_owned())
    }

    /// Adds a key owned by the formatter.
    fn reserved(&mut self, key: &'static str, value: Value) {
        self.entries.push((key.to_string(), value));
    }

    /// Adds a caller-supplied field. Reserved names are moved under
    /// `field.`; a repeated name keeps its last value, so the object never
    /// has duplicate keys.
    fn field(&mut self, key: String, value: Value) {
        let key = if RESERVED_KEYS.contains(&key.as_str()) {
            format!("field.{key}")
        } else {
            key
        };
        if let Some(existing) = self.entries.iter_mut().find(|(name, _)| *name == key) {
            existing.1 = value;
            return;
        }
        self.entries.push((key, value));
    }
}

impl Serialize for JsonLine {
    fn serialize<Ser: Serializer>(&self, serializer: Ser) -> Result<Ser::Ok, Ser::Error> {
        serializer.collect_map(self.entries.iter().map(|(key, value)| (key, value)))
    }
}

/// Whether `c` could break a line or drive a terminal if written raw.
fn is_unsafe_char(c: char) -> bool {
    c.is_control() || matches!(c, '\u{2028}' | '\u{2029}')
}

/// Appends `c` as JSON `\\uXXXX` escapes, one per UTF-16 code unit.
fn push_unicode_escape(out: &mut String, c: char) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for unit in c.encode_utf16(&mut [0; 2]) {
        out.push_str("\\u");
        for shift in [12, 8, 4, 0] {
            out.push(char::from(HEX[usize::from((*unit >> shift) & 0xf)]));
        }
    }
}

/// Escapes what `serde_json` leaves raw: DEL, the C1 controls and the
/// Unicode line/paragraph separators.
///
/// `serde_json` already escapes U+0000..U+001F. In serialised JSON the
/// remaining characters can only occur inside string literals, so replacing
/// them with `\uXXXX` keeps the document valid and the value unchanged.
fn escape_remaining_controls(json: &str) -> Cow<'_, str> {
    if !json.chars().any(is_unsafe_char) {
        return Cow::Borrowed(json);
    }
    let mut escaped = String::with_capacity(json.len() + 16);
    for c in json.chars() {
        if is_unsafe_char(c) {
            push_unicode_escape(&mut escaped, c);
        } else {
            escaped.push(c);
        }
    }
    Cow::Owned(escaped)
}

/// Escapes control characters for the text format, leaving everything else
/// (including non-ASCII text) readable.
fn escape_text(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_unsafe_char) {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len() + 16);
    for c in text.chars() {
        if is_unsafe_char(c) {
            escaped.extend(c.escape_default());
        } else {
            escaped.push(c);
        }
    }
    Cow::Owned(escaped)
}

/// A field value as it appears in a text line: JSON-encoded, so strings are
/// quoted and escaped exactly as in the JSON format.
fn json_text(value: &Value) -> Result<String, fmt::Error> {
    let json = serde_json::to_string(value).map_err(|_| fmt::Error)?;
    Ok(escape_remaining_controls(&json).into_owned())
}

fn level_colour(level: Level) -> &'static str {
    match level {
        Level::ERROR => "\u{1b}[31m",
        Level::WARN => "\u{1b}[33m",
        Level::INFO => "\u{1b}[32m",
        Level::DEBUG => "\u{1b}[34m",
        Level::TRACE => "\u{1b}[35m",
    }
}

fn write_level(writer: &mut Writer<'_>, level: Level) -> fmt::Result {
    if !writer.has_ansi_escapes() {
        return write!(writer, "{level:>5}");
    }
    write!(writer, "{}{level:>5}\u{1b}[0m", level_colour(level))
}

fn write_text_span(writer: &mut Writer<'_>, span: &SpanRecord) -> fmt::Result {
    write!(writer, "{}", span.name)?;
    if !span.fields.is_empty() {
        let mut separator = "{";
        for (key, value) in &span.fields {
            write!(writer, "{separator}{key}={}", json_text(value)?)?;
            separator = " ";
        }
        write!(writer, "}}")?;
    }
    write!(writer, ": ")
}

#[cfg(test)]
pub(crate) mod test_support {
    use chrono::{DateTime, TimeZone, Utc};

    use super::{EventFormatter, LogFormat, LogResource};

    pub(crate) use crate::logging::testing::Sink as Capture;

    pub(crate) fn fixed_clock() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap()
    }

    pub(crate) fn resource() -> LogResource {
        LogResource {
            service_name: "tumult-test".to_string(),
            service_version: "9.9.9".to_string(),
            environment: None,
        }
    }

    pub(crate) fn formatter(format: LogFormat) -> EventFormatter {
        EventFormatter::new(format, resource()).with_clock(fixed_clock)
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::trace::{TraceContextExt, Tracer, TracerProvider};
    use serde_json::{json, Value};
    use tracing_subscriber::layer::SubscriberExt;

    use super::test_support::{formatter, resource, Capture};
    use super::{event_layer, EventFormatter, LogFormat, LogResource};
    use crate::logging::fields::MAX_FIELD_BYTES;

    /// Renders whatever `emit` logs with `formatter` and returns the raw
    /// bytes written, as text.
    fn render_with(formatter: EventFormatter, ansi: bool, emit: impl FnOnce()) -> String {
        let capture = Capture::default();
        let subscriber =
            tracing_subscriber::registry().with(event_layer(formatter, capture.clone(), ansi));
        tracing::subscriber::with_default(subscriber, emit);
        capture.text()
    }

    fn render(format: LogFormat, emit: impl FnOnce()) -> String {
        render_with(formatter(format), false, emit)
    }

    /// Renders one JSON event and parses it, asserting it is exactly one line.
    fn json_event(emit: impl FnOnce()) -> Value {
        let out = render(LogFormat::Json, emit);
        assert_single_line(&out);
        serde_json::from_str(out.trim_end()).unwrap()
    }

    fn assert_single_line(out: &str) {
        assert!(
            out.ends_with('\n'),
            "line must be newline-terminated: {out:?}"
        );
        let body = &out[..out.len() - 1];
        let breaks: Vec<char> = body
            .chars()
            .filter(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
            .collect();
        assert!(
            breaks.is_empty(),
            "raw control characters {breaks:?} in {out:?}"
        );
    }

    /// Runs `inside` with an active OpenTelemetry span and returns that
    /// span's `(trace_id, span_id)` as lowercase hex.
    fn in_otel_span(inside: impl FnOnce()) -> (String, String) {
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let tracer = provider.tracer("format-tests");
        let span = tracer.start("request");
        let cx = opentelemetry::Context::current_with_span(span);
        let ids = {
            let span = cx.span();
            let span_context = span.span_context();
            (
                span_context.trace_id().to_string(),
                span_context.span_id().to_string(),
            )
        };
        let _guard = cx.attach();
        inside();
        ids
    }

    #[test]
    fn json_line_carries_the_required_keys() {
        let event = json_event(|| tracing::info!("daemon started"));
        assert_eq!(event["timestamp"], "2026-01-01T00:00:00.000000Z");
        assert_eq!(event["level"], "INFO");
        assert_eq!(event["target"], module_path!());
        assert_eq!(event["message"], "daemon started");
        assert_eq!(event["service.name"], "tumult-test");
        assert_eq!(event["service.version"], "9.9.9");
    }

    #[test]
    fn json_keys_start_with_timestamp_level_target() {
        let out = render(LogFormat::Json, || tracing::warn!(alpha = 1, "ordered"));
        assert!(
            out.starts_with(
                r#"{"timestamp":"2026-01-01T00:00:00.000000Z","level":"WARN","target":""#
            ),
            "{out}"
        );
    }

    #[test]
    fn json_emits_one_object_per_event() {
        let out = render(LogFormat::Json, || {
            tracing::info!("first");
            tracing::error!("second");
        });
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            assert!(serde_json::from_str::<Value>(line).unwrap().is_object());
        }
    }

    #[test]
    fn json_fields_are_typed() {
        let event = json_event(|| {
            tracing::info!(
                rows = 42_u64,
                ratio = 0.25_f64,
                tls = false,
                file.path = "/var/lib/tumult",
                "export complete"
            );
        });
        assert_eq!(event["rows"], json!(42));
        assert_eq!(event["ratio"], json!(0.25));
        assert_eq!(event["tls"], json!(false));
        assert_eq!(event["file.path"], json!("/var/lib/tumult"));
    }

    #[test]
    fn explicit_event_name_is_emitted() {
        let event = json_event(|| {
            tracing::warn!(name: "auth.login.failed", { user.name = "alice" }, "login failed");
        });
        assert_eq!(event["event.name"], "auth.login.failed");
        assert_eq!(event["message"], "login failed");
    }

    #[test]
    fn unnamed_events_have_no_event_name_key() {
        let event = json_event(|| tracing::info!("unnamed"));
        assert!(event.get("event.name").is_none(), "{event}");
    }

    #[test]
    fn named_event_without_a_message_uses_its_name_as_the_message() {
        let event = json_event(|| tracing::info!(name: "exporter.ready", ready = true));
        assert_eq!(event["message"], "exporter.ready");
    }

    #[test]
    fn injected_newline_in_a_field_yields_exactly_one_line() {
        let hostile = "alice\n{\"level\":\"ERROR\",\"message\":\"forged\"}\r\n";
        let out = render(LogFormat::Json, || {
            tracing::warn!(user.name = %hostile, "login failed");
        });
        assert_eq!(out.matches('\n').count(), 1, "{out:?}");
        assert_single_line(&out);
        let event: Value = serde_json::from_str(out.trim_end()).unwrap();
        assert_eq!(event["user.name"], hostile);
        assert_eq!(event["level"], "WARN");
    }

    #[test]
    fn control_characters_are_escaped_and_round_trip() {
        let hostile = "a\u{0}b\u{1b}[31mc\u{7f}d\u{85}e\u{2028}f\u{2029}g\th";
        let out = render(LogFormat::Json, || {
            tracing::info!(payload = %hostile, "{}", hostile);
        });
        assert_single_line(&out);
        let event: Value = serde_json::from_str(out.trim_end()).unwrap();
        assert_eq!(event["payload"], hostile);
        assert_eq!(event["message"], hostile);
    }

    #[test]
    fn oversized_field_is_truncated_in_the_line() {
        let long = "z".repeat(MAX_FIELD_BYTES + 5);
        let event = json_event(|| tracing::info!(payload = %long, "big"));
        let payload = event["payload"].as_str().unwrap();
        assert!(payload.ends_with("...[truncated 5 bytes]"));
        assert!(payload.len() < MAX_FIELD_BYTES + 64);
    }

    #[test]
    fn trace_and_span_ids_are_present_inside_an_otel_span() {
        let mut ids = (String::new(), String::new());
        let event = json_event(|| {
            ids = in_otel_span(|| tracing::info!("inside"));
        });
        assert_eq!(event["trace_id"], json!(ids.0));
        assert_eq!(event["span_id"], json!(ids.1));
        assert_eq!(ids.0.len(), 32);
        assert_eq!(ids.1.len(), 16);
    }

    #[test]
    fn trace_and_span_ids_are_absent_outside_a_span() {
        let event = json_event(|| tracing::info!("outside"));
        assert!(event.get("trace_id").is_none(), "{event}");
        assert!(event.get("span_id").is_none(), "{event}");
    }

    #[test]
    fn a_field_cannot_overwrite_a_reserved_key() {
        let event = json_event(|| {
            tracing::info!(
                level = "FORGED",
                trace_id = "0",
                timestamp = "never",
                "reserved"
            );
        });
        assert_eq!(event["level"], "INFO");
        assert_eq!(event["timestamp"], "2026-01-01T00:00:00.000000Z");
        assert!(event.get("trace_id").is_none());
        assert_eq!(event["field.level"], "FORGED");
        assert_eq!(event["field.trace_id"], "0");
        assert_eq!(event["field.timestamp"], "never");
    }

    #[test]
    fn enclosing_tracing_spans_are_listed_root_first_with_typed_fields() {
        let event = json_event(|| {
            let outer = tracing::info_span!("gameday", round = 2_u64);
            let _outer = outer.enter();
            let inner = tracing::info_span!("ssh.exec", host = "db-1");
            let _inner = inner.enter();
            tracing::info!("command sent");
        });
        assert_eq!(
            event["spans"],
            json!([{"name": "gameday", "round": 2}, {"name": "ssh.exec", "host": "db-1"}])
        );
    }

    #[test]
    fn deployment_environment_is_emitted_when_known() {
        let resource = LogResource {
            environment: Some("prod".to_string()),
            ..resource()
        };
        let formatter = EventFormatter::new(LogFormat::Json, resource)
            .with_clock(super::test_support::fixed_clock);
        let out = render_with(formatter, false, || tracing::info!("env"));
        let event: Value = serde_json::from_str(out.trim_end()).unwrap();
        assert_eq!(event["deployment.environment.name"], "prod");
        let without = json_event(|| tracing::info!("env"));
        assert!(without.get("deployment.environment.name").is_none());
    }

    #[test]
    fn text_line_is_readable_and_carries_fields() {
        let out = render(LogFormat::Text, || {
            tracing::warn!(name: "auth.login.failed", { user.name = "alice", attempt = 3 }, "login failed");
        });
        assert_eq!(
            out,
            format!(
                "2026-01-01T00:00:00.000000Z  WARN {}: login failed event.name=auth.login.failed user.name=\"alice\" attempt=3\n",
                module_path!()
            )
        );
    }

    #[test]
    fn text_line_is_not_json() {
        let out = render(LogFormat::Text, || tracing::info!("plain"));
        assert!(serde_json::from_str::<Value>(out.trim_end()).is_err());
    }

    #[test]
    fn text_line_ends_with_the_trace_ids_inside_an_otel_span() {
        let mut ids = (String::new(), String::new());
        let out = render(LogFormat::Text, || {
            ids = in_otel_span(|| tracing::info!("inside"));
        });
        assert!(
            out.ends_with(&format!(" trace_id={} span_id={}\n", ids.0, ids.1)),
            "{out}"
        );
    }

    #[test]
    fn text_line_escapes_control_characters_in_message_and_fields() {
        let hostile = "alice\nFORGED\u{1b}[0m\u{2028}";
        let out = render(LogFormat::Text, || {
            tracing::warn!(user.name = %hostile, "{}", hostile);
        });
        assert_eq!(out.matches('\n').count(), 1, "{out:?}");
        assert_single_line(&out);
        assert!(out.contains(r"alice\nFORGED\u{1b}[0m\u{2028} "), "{out}");
        assert!(
            out.contains(r#"user.name="alice\nFORGED\u001b[0m\u2028""#),
            "{out}"
        );
    }

    #[test]
    fn text_line_shows_enclosing_spans() {
        let out = render(LogFormat::Text, || {
            let span = tracing::info_span!("ssh.exec", host = "db-1", port = 22_u16);
            let _entered = span.enter();
            tracing::info!("command sent");
        });
        assert!(
            out.contains(r#": ssh.exec{host="db-1" port=22}: command sent"#),
            "{out}"
        );
    }

    #[test]
    fn text_level_is_coloured_only_when_ansi_is_enabled() {
        let coloured = render_with(formatter(LogFormat::Text), true, || tracing::error!("x"));
        assert!(
            coloured.contains("\u{1b}[31mERROR\u{1b}[0m"),
            "{coloured:?}"
        );
        let plain = render_with(formatter(LogFormat::Text), false, || tracing::error!("x"));
        assert!(!plain.contains('\u{1b}'));
    }

    #[test]
    fn json_never_contains_ansi_even_when_the_writer_allows_it() {
        let out = render_with(formatter(LogFormat::Json), true, || tracing::error!("x"));
        assert!(!out.contains('\u{1b}'));
    }

    #[test]
    fn unicode_escape_uses_four_hex_digits_per_utf16_unit() {
        let mut out = String::new();
        super::push_unicode_escape(&mut out, '\u{7f}');
        super::push_unicode_escape(&mut out, '\u{2028}');
        super::push_unicode_escape(&mut out, '\u{1f600}');
        assert_eq!(out, r"\u007f\u2028\ud83d\ude00");
        let decoded: String = serde_json::from_str(&format!("\"{out}\"")).unwrap();
        assert_eq!(decoded, "\u{7f}\u{2028}\u{1f600}");
    }

    #[test]
    fn a_direct_error_line_is_shaped_like_a_formatted_event() {
        let event = super::DirectEvent {
            name: "process.fatal",
            message: "exiting because of an unrecoverable error",
            fields: vec![
                ("error.message", json!("open store: locked\nby pid 7")),
                ("level", json!("spoofed")),
            ],
        };
        let line = super::error_line(
            &LogResource {
                environment: Some("prod".to_string()),
                ..resource()
            },
            super::test_support::fixed_clock(),
            "tumult_otel::logging",
            event,
        );
        assert!(!line.contains('\n'), "{line}");
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(
            parsed,
            json!({
                "timestamp": "2026-01-01T00:00:00.000000Z",
                "level": "ERROR",
                "target": "tumult_otel::logging",
                "event.name": "process.fatal",
                "message": "exiting because of an unrecoverable error",
                "service.name": "tumult-test",
                "service.version": "9.9.9",
                "deployment.environment.name": "prod",
                "error.message": "open store: locked\nby pid 7",
                "field.level": "spoofed",
            })
        );
    }
}
