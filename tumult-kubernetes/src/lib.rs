//! Tumult Kubernetes — Native K8s chaos actions and probes.
//!
//! Uses [`kube`](https://docs.rs/kube) (kube-rs) for async Kubernetes API
//! access. All actions and probes operate through the standard Kubernetes API
//! server, so no privileged DaemonSet is required.
//!
//! # Supported chaos actions
//!
//! - **Pod deletion** — immediate or graceful (`delete_pod`)
//! - **Node drain** — cordon + evict pods (`drain_node`)
//! - **Deployment scaling** — scale replicas up or down (`scale_deployment`)
//! - **Network policy** — apply restrictive NetworkPolicy to simulate partition
//!
//! # Probes
//!
//! - Pod readiness and phase checks
//! - Deployment available-replica counts
//! - Node condition inspection
//! - Service endpoint enumeration
//!
//! # Authentication
//!
//! `kube-rs` automatically discovers credentials from the in-cluster service
//! account, `KUBECONFIG`, or `~/.kube/config`. No extra configuration is
//! needed when running inside a cluster.

pub mod actions;
pub mod error;
pub mod probes;
pub(crate) mod telemetry;

pub use error::KubeError;
