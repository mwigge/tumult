//! Newtype identifiers for OpenTelemetry trace and span IDs.

use serde::{Deserialize, Serialize};

/// A newtype wrapper for OpenTelemetry trace IDs.
///
/// Stored as a hex string (e.g. `"4bf92f3577b34da6a3ce929d0e0e4736"`).
/// Empty string signals no active trace (noop tracer or uninstrumented path).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TraceId(pub String);

impl TraceId {
    /// Creates an empty (no-trace) identifier.
    #[must_use]
    pub fn empty() -> Self {
        Self(String::new())
    }

    /// Returns `true` if the trace ID is empty (noop tracer or uninstrumented).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the inner string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for TraceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for TraceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TraceId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl AsRef<str> for TraceId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// A newtype wrapper for OpenTelemetry span IDs.
///
/// Stored as a hex string (e.g. `"00f067aa0ba902b7"`).
/// Empty string signals no active span (noop tracer or uninstrumented path).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpanId(pub String);

impl SpanId {
    /// Creates an empty (no-span) identifier.
    #[must_use]
    pub fn empty() -> Self {
        Self(String::new())
    }

    /// Returns `true` if the span ID is empty (noop tracer or uninstrumented).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the inner string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SpanId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for SpanId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for SpanId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl AsRef<str> for SpanId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_ids_signal_an_uninstrumented_path() {
        assert!(TraceId::empty().is_empty());
        assert!(SpanId::empty().is_empty());
        assert_eq!(TraceId::default(), TraceId::empty());
        assert_eq!(SpanId::default(), SpanId::empty());
    }

    #[test]
    fn ids_expose_the_inner_hex_string() {
        let trace = TraceId::from("4bf92f3577b34da6a3ce929d0e0e4736");
        assert!(!trace.is_empty());
        assert_eq!(trace.as_str(), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(trace.to_string(), "4bf92f3577b34da6a3ce929d0e0e4736");
        let as_ref: &str = trace.as_ref();
        assert_eq!(as_ref, "4bf92f3577b34da6a3ce929d0e0e4736");

        let span = SpanId::from("00f067aa0ba902b7".to_string());
        assert!(!span.is_empty());
        assert_eq!(span.as_str(), "00f067aa0ba902b7");
        assert_eq!(span.to_string(), "00f067aa0ba902b7");
        let as_ref: &str = span.as_ref();
        assert_eq!(as_ref, "00f067aa0ba902b7");
    }

    #[test]
    fn ids_serialize_transparently_as_plain_strings() {
        let trace = TraceId::from("abc");
        let json = serde_json::to_string(&trace).expect("serialize");
        assert_eq!(json, "\"abc\"");
        let decoded: TraceId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, trace);

        let span = SpanId::from("def");
        let json = serde_json::to_string(&span).expect("serialize");
        assert_eq!(json, "\"def\"");
        let decoded: SpanId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, span);
    }
}
