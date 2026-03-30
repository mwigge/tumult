//! Kubernetes error types.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum KubeError {
    #[error("kubernetes API error: {0}")]
    Api(#[from] kube::Error),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
}
