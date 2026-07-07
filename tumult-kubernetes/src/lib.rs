//! Tumult Kubernetes — Native K8s chaos actions and probes.
//!
//! Uses [`kube`](https://docs.rs/kube) (kube-rs) for async Kubernetes API
//! access. All actions and probes operate through the standard Kubernetes API
//! server, so no privileged `DaemonSet` is required.
//!
//! # Supported chaos actions
//!
//! ## Control plane (via the API server only)
//!
//! - **Pod deletion** — immediate or graceful (`delete_pod`)
//! - **Node drain** — cordon + evict pods (`drain_node`)
//! - **Deployment scaling** — scale replicas up or down (`scale_deployment`)
//! - **Network policy** — apply restrictive `NetworkPolicy` to simulate partition
//!
//! ## In-pod data plane (via ephemeral debug containers — [`inject`])
//!
//! - **Pod network latency** — `tc netem` from an ephemeral container sharing
//!   the target pod's network namespace (`pod_network_latency`)
//! - **Pod stress** — `stress-ng` (CPU or memory) in the target container's
//!   process namespace (`pod_stress`)
//!
//! These close the data-plane gap versus Chaos Mesh / `LitmusChaos` **without** a
//! permanent privileged `DaemonSet`: the injected container is short-lived and
//! self-terminating. See the [`inject`] module docs for the lifecycle
//! limits (ephemeral containers are GA since 1.25 and cannot be removed once
//! attached).
//!
//! # Topology discovery ([`discovery`])
//!
//! Lists Services and renders a *proposed* topology TOML for human review —
//! discovery never writes the graph directly (topology is declared, not
//! guessed).
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
pub mod discovery;
pub mod error;
pub mod inject;
pub mod native;
pub mod probes;
pub(crate) mod telemetry;

pub use error::KubeError;
pub use native::KubernetesExecutor;
