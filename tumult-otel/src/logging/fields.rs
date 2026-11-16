//! Typed field capture for log events and spans.
//!
//! Every value recorded on a `tracing` event or span is converted to a
//! [`serde_json::Value`] of the matching JSON type. String values are capped
//! at [`MAX_FIELD_BYTES`] so a single attacker-controlled or runaway value
//! cannot produce an unbounded log line.

use std::borrow::Cow;
use std::fmt;

use serde_json::{Map, Number, Value};
use tracing::field::{Field, Visit};
use tracing_subscriber::field::RecordFields;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FormatFields, FormattedFields};

/// Upper bound, in bytes, for any single string value in a log line (8 KiB).
///
/// Longer values are cut at the last character boundary at or below this
/// length and suffixed with `...[truncated N bytes]`.
pub const MAX_FIELD_BYTES: usize = 8 * 1024;

/// The name `tracing` gives the field that carries an event's message.
const MESSAGE_FIELD: &str = "message";

/// Caps `value` at [`MAX_FIELD_BYTES`], appending a marker that states how
/// many bytes were dropped.
#[must_use]
pub fn bounded(value: &str) -> Cow<'_, str> {
    if value.len() <= MAX_FIELD_BYTES {
        return Cow::Borrowed(value);
    }
    let mut end = MAX_FIELD_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let dropped = value.len() - end;
    Cow::Owned(format!("{}...[truncated {dropped} bytes]", &value[..end]))
}

fn string_value(value: &str) -> Value {
    Value::String(bounded(value).into_owned())
}

fn float_value(value: f64) -> Value {
    Number::from_f64(value).map_or_else(|| Value::String(value.to_string()), Value::Number)
}

/// A `tracing` visitor that collects an event's message and its fields as
/// typed JSON values, in the order they were recorded.
#[derive(Debug, Default)]
pub(crate) struct FieldCollector {
    pub(crate) message: Option<String>,
    pub(crate) fields: Vec<(String, Value)>,
}

impl FieldCollector {
    fn push(&mut self, field: &Field, value: Value) {
        self.fields.push((field.name().to_string(), value));
    }

    fn push_text(&mut self, field: &Field, text: &str) {
        if field.name() == MESSAGE_FIELD {
            self.message = Some(bounded(text).into_owned());
            return;
        }
        self.push(field, string_value(text));
    }
}

impl Visit for FieldCollector {
    fn record_f64(&mut self, field: &Field, value: f64) {
        self.push(field, float_value(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.push(field, Value::from(value));
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.push(field, Value::from(value));
    }

    fn record_i128(&mut self, field: &Field, value: i128) {
        let json =
            i64::try_from(value).map_or_else(|_| Value::String(value.to_string()), Value::from);
        self.push(field, json);
    }

    fn record_u128(&mut self, field: &Field, value: u128) {
        let json =
            u64::try_from(value).map_or_else(|_| Value::String(value.to_string()), Value::from);
        self.push(field, json);
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.push(field, Value::Bool(value));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.push_text(field, value);
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.push_text(field, &value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.push_text(field, &format!("{value:?}"));
    }
}

/// A [`FormatFields`] implementation that stores a span's fields as one JSON
/// object with typed values, so the event formatters can emit them without
/// re-parsing `key=value` text.
#[derive(Debug, Clone, Copy, Default)]
pub struct TypedFields;

impl TypedFields {
    fn collect<R: RecordFields>(fields: R) -> Map<String, Value> {
        let mut collector = FieldCollector::default();
        fields.record(&mut collector);
        collector.fields.into_iter().collect()
    }

    fn write(mut writer: Writer<'_>, object: &Map<String, Value>) -> fmt::Result {
        let json = serde_json::to_string(object).map_err(|_| fmt::Error)?;
        writer.write_str(&json)
    }
}

impl<'writer> FormatFields<'writer> for TypedFields {
    fn format_fields<R: RecordFields>(&self, writer: Writer<'writer>, fields: R) -> fmt::Result {
        Self::write(writer, &Self::collect(fields))
    }

    /// Merges values recorded after span creation into the stored object
    /// (the default implementation would append text and break the JSON).
    fn add_fields(
        &self,
        current: &'writer mut FormattedFields<Self>,
        fields: &tracing::span::Record<'_>,
    ) -> fmt::Result {
        let mut object: Map<String, Value> = if current.fields.is_empty() {
            Map::new()
        } else {
            serde_json::from_str(&current.fields).map_err(|_| fmt::Error)?
        };
        object.extend(Self::collect(fields));
        current.fields.clear();
        Self::write(current.as_writer(), &object)
    }
}

#[cfg(test)]
mod tests {
    use super::{bounded, FieldCollector, TypedFields, MAX_FIELD_BYTES};
    use serde_json::{json, Value};

    /// Runs `emit` under a subscriber that captures the fields of the one
    /// event it produces.
    fn collect(emit: impl FnOnce()) -> FieldCollector {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::layer::SubscriberExt;

        struct Capture(Arc<Mutex<Option<FieldCollector>>>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Capture {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                let mut collector = FieldCollector::default();
                event.record(&mut collector);
                *self.0.lock().unwrap() = Some(collector);
            }
        }

        let slot = Arc::new(Mutex::new(None));
        let subscriber = tracing_subscriber::registry().with(Capture(Arc::clone(&slot)));
        tracing::subscriber::with_default(subscriber, emit);
        let collected = slot.lock().unwrap().take();
        collected.expect("one event")
    }

    fn field<'a>(collector: &'a FieldCollector, key: &str) -> &'a Value {
        collector
            .fields
            .iter()
            .find(|(name, _)| name == key)
            .map_or_else(|| panic!("field {key} missing"), |(_, value)| value)
    }

    #[test]
    fn values_keep_their_json_type() {
        let collector = collect(|| {
            tracing::info!(
                count = 3_u64,
                delta = -4_i64,
                ratio = 0.5_f64,
                ok = true,
                name = "alice",
                shown = %"display",
                debugged = ?Some(1),
                "typed"
            );
        });
        assert_eq!(collector.message.as_deref(), Some("typed"));
        assert_eq!(field(&collector, "count"), &json!(3));
        assert_eq!(field(&collector, "delta"), &json!(-4));
        assert_eq!(field(&collector, "ratio"), &json!(0.5));
        assert_eq!(field(&collector, "ok"), &json!(true));
        assert_eq!(field(&collector, "name"), &json!("alice"));
        assert_eq!(field(&collector, "shown"), &json!("display"));
        assert_eq!(field(&collector, "debugged"), &json!("Some(1)"));
    }

    #[test]
    fn dotted_and_quoted_field_names_are_kept_verbatim() {
        let collector = collect(|| {
            tracing::warn!(error.message = %"boom", "error.type" = "io", "failed");
        });
        assert_eq!(field(&collector, "error.message"), &json!("boom"));
        assert_eq!(field(&collector, "error.type"), &json!("io"));
    }

    #[test]
    fn non_finite_floats_become_strings() {
        let collector = collect(|| tracing::info!(ratio = f64::NAN, "nan"));
        assert_eq!(field(&collector, "ratio"), &json!("NaN"));
    }

    #[test]
    fn wide_integers_outside_the_json_range_become_strings() {
        let collector = collect(|| tracing::info!(big = u128::MAX, small = 7_u128, "wide"));
        assert_eq!(field(&collector, "big"), &json!(u128::MAX.to_string()));
        assert_eq!(field(&collector, "small"), &json!(7));
    }

    #[test]
    fn error_values_are_recorded_by_display() {
        let error = std::io::Error::other("disk on fire");
        let collector = collect(|| {
            tracing::error!(
                error.message = &error as &(dyn std::error::Error + 'static),
                "failed"
            );
        });
        assert_eq!(field(&collector, "error.message"), &json!("disk on fire"));
    }

    #[test]
    fn short_strings_are_untouched() {
        assert_eq!(bounded("hello"), "hello");
        let exact = "x".repeat(MAX_FIELD_BYTES);
        assert_eq!(bounded(&exact), exact);
    }

    #[test]
    fn long_strings_are_cut_with_a_marker_naming_the_dropped_bytes() {
        let long = "x".repeat(MAX_FIELD_BYTES + 100);
        let cut = bounded(&long);
        assert!(cut.starts_with(&"x".repeat(MAX_FIELD_BYTES)));
        assert!(cut.ends_with("...[truncated 100 bytes]"));
        assert_eq!(
            cut.len(),
            MAX_FIELD_BYTES + "...[truncated 100 bytes]".len()
        );
    }

    #[test]
    fn truncation_never_splits_a_multibyte_character() {
        // 'é' is two bytes; with the leading 'a' the cap lands inside one.
        let long = format!("a{}", "é".repeat(MAX_FIELD_BYTES));
        let cut = bounded(&long);
        let kept = cut.split("...[truncated").next().unwrap();
        assert_eq!(kept.len(), MAX_FIELD_BYTES - 1);
        assert!(kept.chars().skip(1).all(|c| c == 'é'));
    }

    #[test]
    fn oversized_event_fields_and_messages_are_truncated() {
        let long = "y".repeat(MAX_FIELD_BYTES * 2);
        let collector = collect(|| tracing::info!(payload = %long, "{}", long));
        let payload = field(&collector, "payload").as_str().unwrap();
        assert!(payload.contains("...[truncated"));
        assert!(collector.message.unwrap().contains("...[truncated"));
    }

    /// Returns the JSON text [`TypedFields`] stored for the span `make`
    /// creates, as seen when that span closes.
    fn stored_span_fields(make: impl FnOnce() -> tracing::Span) -> String {
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::FormattedFields;
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::registry::LookupSpan;

        struct Probe(Arc<Mutex<String>>);
        impl<S> tracing_subscriber::Layer<S> for Probe
        where
            S: tracing::Subscriber + for<'a> LookupSpan<'a>,
        {
            fn on_close(
                &self,
                id: tracing::span::Id,
                ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                let span = ctx.span(&id).expect("span");
                let extensions = span.extensions();
                let fields = extensions
                    .get::<FormattedFields<TypedFields>>()
                    .expect("formatted fields");
                self.0.lock().unwrap().clone_from(&fields.fields);
            }
        }

        let slot = Arc::new(Mutex::new(String::new()));
        // The probe sits below the fmt layer so it observes `on_close`
        // before the fmt layer's stored fields are dropped with the span.
        let subscriber = tracing_subscriber::registry()
            .with(Probe(Arc::clone(&slot)))
            .with(
                tracing_subscriber::fmt::layer()
                    .fmt_fields(TypedFields)
                    .with_writer(std::io::sink),
            );
        tracing::subscriber::with_default(subscriber, || drop(make()));
        let stored = slot.lock().unwrap().clone();
        stored
    }

    #[test]
    fn span_fields_are_stored_as_a_typed_json_object() {
        let stored =
            stored_span_fields(|| tracing::info_span!("ssh.exec", host = "db-1", port = 22_u16));
        let parsed: Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed, json!({"host": "db-1", "port": 22}));
    }

    #[test]
    fn fields_recorded_later_are_merged_into_the_same_object() {
        let stored = stored_span_fields(|| {
            let span = tracing::info_span!("op", host = "db-1", exit_code = tracing::field::Empty);
            span.record("exit_code", 7_i64);
            span
        });
        let parsed: Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(parsed, json!({"host": "db-1", "exit_code": 7}));
    }
}
