//! Kubernetes error types.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum KubeError {
    #[error("kubernetes API error: {0}")]
    Api(#[from] kube::Error),

    /// A configuration field was invalid.
    ///
    /// Use this variant when callers need to distinguish *which* field failed;
    /// it allows programmatic matching on `field` without parsing the message string.
    #[error("invalid configuration: field `{field}` — {reason}")]
    InvalidConfig {
        /// The name of the configuration field that was invalid.
        field: &'static str,
        /// Human-readable explanation of why the value is invalid.
        reason: String,
    },
}
