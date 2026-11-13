//! The size cap on exported log records.
//!
//! The local formatter caps every string at
//! [`MAX_FIELD_BYTES`](super::MAX_FIELD_BYTES) through
//! [`bounded`]. The OTLP bridge reads an event's fields itself, so the same
//! cap is applied here, on the record it fills in: a logger provider whose
//! records pass every string through `bounded` before storing it.

use std::borrow::Cow;
use std::time::SystemTime;

use opentelemetry::logs::{AnyValue, LogRecord, Logger, LoggerProvider, Severity};
use opentelemetry::{InstrumentationScope, Key, SpanId, TraceFlags, TraceId};
use opentelemetry_sdk::logs::{SdkLogRecord, SdkLogger, SdkLoggerProvider};

use super::fields::{bounded, MAX_FIELD_BYTES};

/// `value` with a string capped by [`bounded`] and a byte string cut at the
/// same length; every other value is returned as it is.
fn capped(value: AnyValue) -> AnyValue {
    match value {
        AnyValue::String(text) => match bounded(text.as_str()) {
            Cow::Borrowed(_) => AnyValue::String(text),
            Cow::Owned(shortened) => AnyValue::String(shortened.into()),
        },
        AnyValue::Bytes(mut bytes) => {
            bytes.truncate(MAX_FIELD_BYTES);
            AnyValue::Bytes(bytes)
        }
        other => other,
    }
}

/// A logger provider whose records are capped; see the
/// [module documentation](self).
pub(crate) struct CappedProvider(SdkLoggerProvider);

impl CappedProvider {
    pub(crate) fn new(provider: SdkLoggerProvider) -> Self {
        Self(provider)
    }
}

impl LoggerProvider for CappedProvider {
    type Logger = CappedLogger;

    fn logger_with_scope(&self, scope: InstrumentationScope) -> Self::Logger {
        CappedLogger(self.0.logger_with_scope(scope))
    }
}

/// The logger of a [`CappedProvider`].
pub(crate) struct CappedLogger(SdkLogger);

impl Logger for CappedLogger {
    type LogRecord = CappedRecord;

    fn create_log_record(&self) -> Self::LogRecord {
        CappedRecord(self.0.create_log_record())
    }

    fn emit(&self, record: Self::LogRecord) {
        self.0.emit(record.0);
    }

    fn event_enabled(&self, level: Severity, target: &str, name: Option<&str>) -> bool {
        self.0.event_enabled(level, target, name)
    }
}

/// A log record that caps its body and attributes as they are set.
pub(crate) struct CappedRecord(SdkLogRecord);

impl LogRecord for CappedRecord {
    fn set_event_name(&mut self, name: &'static str) {
        self.0.set_event_name(name);
    }

    fn set_target<T>(&mut self, target: T)
    where
        T: Into<Cow<'static, str>>,
    {
        self.0.set_target(target);
    }

    fn set_timestamp(&mut self, timestamp: SystemTime) {
        self.0.set_timestamp(timestamp);
    }

    fn set_observed_timestamp(&mut self, timestamp: SystemTime) {
        self.0.set_observed_timestamp(timestamp);
    }

    fn set_severity_text(&mut self, text: &'static str) {
        self.0.set_severity_text(text);
    }

    fn set_severity_number(&mut self, number: Severity) {
        self.0.set_severity_number(number);
    }

    fn set_body(&mut self, body: AnyValue) {
        self.0.set_body(capped(body));
    }

    fn add_attributes<I, K, V>(&mut self, attributes: I)
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<Key>,
        V: Into<AnyValue>,
    {
        for (key, value) in attributes {
            self.add_attribute(key, value);
        }
    }

    fn add_attribute<K, V>(&mut self, key: K, value: V)
    where
        K: Into<Key>,
        V: Into<AnyValue>,
    {
        self.0.add_attribute(key, capped(value.into()));
    }

    fn set_trace_context(
        &mut self,
        trace_id: TraceId,
        span_id: SpanId,
        trace_flags: Option<TraceFlags>,
    ) {
        self.0.set_trace_context(trace_id, span_id, trace_flags);
    }
}

#[cfg(test)]
mod tests {
    use opentelemetry::logs::AnyValue;

    use super::capped;
    use crate::logging::fields::{bounded, MAX_FIELD_BYTES};

    #[test]
    fn a_long_string_is_cut_exactly_as_the_local_formatter_cuts_it() {
        let long = "\u{e9}".repeat(MAX_FIELD_BYTES);
        let AnyValue::String(cut) = capped(AnyValue::from(long.clone())) else {
            panic!("a string stays a string");
        };
        assert_eq!(cut.as_str(), bounded(&long));
    }

    #[test]
    fn long_bytes_are_cut_at_the_same_length() {
        let AnyValue::Bytes(cut) = capped(AnyValue::Bytes(Box::new(vec![7; MAX_FIELD_BYTES + 9])))
        else {
            panic!("bytes stay bytes");
        };
        assert_eq!(cut.len(), MAX_FIELD_BYTES);
    }

    #[test]
    fn values_within_the_cap_are_untouched() {
        assert_eq!(capped(AnyValue::from("short")), AnyValue::from("short"));
        assert_eq!(capped(AnyValue::Int(7)), AnyValue::Int(7));
        assert_eq!(capped(AnyValue::Boolean(true)), AnyValue::Boolean(true));
    }
}
