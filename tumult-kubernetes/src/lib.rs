//! Tumult Kubernetes — Native K8s chaos actions and probes.
//!
//! Uses `kube-rs` for async Kubernetes API access. Supports:
//! - Pod deletion (immediate and graceful)
//! - Node drain (cordon + evict pods)
//! - Deployment scaling
//! - Network policy application
//! - Status probes for pods, deployments, nodes, services

pub mod actions;
pub mod error;
pub mod probes;

pub use error::KubeError;
