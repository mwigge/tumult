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
