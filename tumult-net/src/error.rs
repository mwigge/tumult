//! Network chaos error types.

use thiserror::Error;

/// Errors raised by the TCP chaos proxy actions and probes.
#[derive(Error, Debug)]
pub enum NetError {
    /// An underlying I/O operation failed (bind, connect, accept, spawn, …).
    #[error("network I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// A configuration field held an invalid value.
    ///
    /// Use this variant when callers need to distinguish *which* field failed;
    /// it allows programmatic matching on `field` without parsing the message.
    #[error("invalid configuration: field `{field}` — {reason}")]
    InvalidConfig {
        /// The name of the configuration field that was invalid.
        field: &'static str,
        /// Human-readable explanation of why the value is invalid.
        reason: String,
    },
}

impl NetError {
    /// Construct an [`NetError::InvalidConfig`] from a static field name and an
    /// owned reason string.
    #[must_use]
    pub fn invalid(field: &'static str, reason: impl Into<String>) -> Self {
        Self::InvalidConfig {
            field,
            reason: reason.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::NetError;

    #[test]
    fn invalid_config_message_contains_field() {
        let err = NetError::invalid("delay_ms", "must be >= 0");
        assert!(err.to_string().contains("delay_ms"));
        assert!(err.to_string().contains("must be >= 0"));
    }

    #[test]
    fn io_error_converts_via_from() {
        let io = std::io::Error::new(std::io::ErrorKind::AddrInUse, "boom");
        let err: NetError = io.into();
        assert!(matches!(err, NetError::Io(_)));
    }
}
