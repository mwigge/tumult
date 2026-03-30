//! Kubernetes error types.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum KubeError {
    #[error("kubernetes API error: {0}")]
    Api(#[from] kube::Error),

    #[error("pod not found: {namespace}/{name}")]
    PodNotFound { namespace: String, name: String },

    #[error("deployment not found: {namespace}/{name}")]
    DeploymentNotFound { namespace: String, name: String },

    #[error("node not found: {name}")]
    NodeNotFound { name: String },

    #[error("operation timed out after {seconds}s")]
    Timeout { seconds: f64 },

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
}
