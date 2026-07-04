//! Cloud connector error types.
//!
//! Every provider failure maps to a typed [`CloudError`] variant so the native
//! dispatch layer never sees a panic — HTTP transport errors, authentication
//! failures, not-found, throttling, and generic API errors are all
//! distinguishable by the caller.

use thiserror::Error;

/// Errors raised by the cloud fault connectors.
#[derive(Error, Debug)]
pub enum CloudError {
    /// A required credential environment variable was not set. Raised
    /// *before* any network call so the fail-fast message names the exact
    /// variable and how to obtain it.
    #[error("missing credential: {var} is not set ({context})")]
    MissingCredential {
        /// The environment variable that was absent.
        var: &'static str,
        /// Guidance on the credential chain / how to obtain the value.
        context: &'static str,
    },

    /// A configuration field (region, endpoint, id) held an invalid value.
    #[error("invalid configuration: field `{field}` — {reason}")]
    InvalidConfig {
        /// The name of the field that was invalid.
        field: &'static str,
        /// Human-readable explanation of why the value is invalid.
        reason: String,
    },

    /// The HTTP request never completed (DNS, TLS, connection reset, …).
    #[error("HTTP transport error: {0}")]
    Transport(String),

    /// The provider rejected the request for authentication or authorization
    /// reasons (HTTP 401 / 403).
    #[error("authentication failed (HTTP {status}): {message}")]
    Auth {
        /// The HTTP status code returned.
        status: u16,
        /// The provider's error body.
        message: String,
    },

    /// The addressed resource does not exist (HTTP 404).
    #[error("resource not found (HTTP 404): {message}")]
    NotFound {
        /// The provider's error body.
        message: String,
    },

    /// The provider throttled the request (HTTP 429, or an AWS
    /// `ThrottlingException` on a 400).
    #[error("request throttled (HTTP {status}): {message}")]
    Throttled {
        /// The HTTP status code returned.
        status: u16,
        /// The provider's error body.
        message: String,
    },

    /// Any other non-success response from the provider API.
    #[error("{provider} API error (HTTP {status}): {message}")]
    Api {
        /// The provider name (`aws`, `azure`, `gcp`).
        provider: &'static str,
        /// The HTTP status code returned.
        status: u16,
        /// The provider's error body.
        message: String,
    },
}

impl CloudError {
    /// Classify a non-2xx provider response into the most specific variant.
    ///
    /// `401`/`403` map to [`CloudError::Auth`], `404` to
    /// [`CloudError::NotFound`], `429` (and any body advertising throttling)
    /// to [`CloudError::Throttled`]; everything else falls through to
    /// [`CloudError::Api`].
    #[must_use]
    pub fn from_status(provider: &'static str, status: u16, body: &str) -> Self {
        let looks_throttled = body.contains("Throttl")
            || body.contains("TooManyRequests")
            || body.contains("RequestLimitExceeded");
        match status {
            401 | 403 => Self::Auth {
                status,
                message: body.to_string(),
            },
            404 => Self::NotFound {
                message: body.to_string(),
            },
            429 => Self::Throttled {
                status,
                message: body.to_string(),
            },
            _ if looks_throttled => Self::Throttled {
                status,
                message: body.to_string(),
            },
            _ => Self::Api {
                provider,
                status,
                message: body.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CloudError;

    #[test]
    fn status_classification_is_typed() {
        assert!(matches!(
            CloudError::from_status("aws", 403, "no"),
            CloudError::Auth { .. }
        ));
        assert!(matches!(
            CloudError::from_status("azure", 404, "no"),
            CloudError::NotFound { .. }
        ));
        assert!(matches!(
            CloudError::from_status("aws", 429, "slow down"),
            CloudError::Throttled { .. }
        ));
        assert!(matches!(
            CloudError::from_status("aws", 400, "ThrottlingException"),
            CloudError::Throttled { .. }
        ));
        assert!(matches!(
            CloudError::from_status("gcp", 500, "boom"),
            CloudError::Api { .. }
        ));
    }
}
