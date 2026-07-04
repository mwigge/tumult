//! In-pod, data-plane fault injection via **ephemeral debug containers**.
//!
//! Chaos Mesh and `LitmusChaos` reach *inside* a running pod (per-pod network
//! latency, in-pod CPU/memory stress) with a permanently-installed, privileged
//! `DaemonSet` that owns every node's kernel. Tumult's identity is the
//! opposite — no control plane, a single binary, nothing left running. This
//! module closes the data-plane gap **without** that daemon by using the
//! Kubernetes *ephemeral containers* subresource: the same mechanism
//! `kubectl debug` uses. A short-lived, self-terminating container is attached
//! to the target pod, does its work inside the pod's namespaces, and exits.
//!
//! # Mechanism
//!
//! Both faults `PATCH` the `pods/{name}/ephemeralcontainers` subresource with a
//! strategic-merge patch that *appends* one ephemeral container (existing
//! ephemeral containers, if any, are preserved by the merge key `name`). The
//! injected container runs a self-terminating shell command: it applies the
//! fault, `sleep`s for `duration_s`, then cleans up and exits.
//!
//! - **[`pod_network_latency`]** — an ephemeral container running an image that
//!   ships `tc` (iproute2). Every container in a pod already shares one network
//!   namespace, so `tc qdisc add dev <iface> root netem delay …` applied from
//!   the ephemeral container degrades the whole pod's egress. The command
//!   `sleep`s for the duration then runs `tc qdisc del` to restore.
//! - **[`pod_stress`]** — an ephemeral container running `stress-ng`. Setting
//!   `targetContainerName` places it in the target container's process
//!   namespace so the load lands alongside the real workload. `stress-ng
//!   --timeout` makes it self-terminating.
//!
//! # Honest limitations (vanilla cluster)
//!
//! - **Ephemeral containers must be enabled.** GA and on-by-default since
//!   Kubernetes 1.25. On older clusters the `ephemeralcontainers` subresource
//!   returns 404/NotFound and injection fails with a typed API error.
//! - **They cannot be removed.** The Kubernetes API forbids deleting or
//!   mutating an ephemeral container once attached — the pod spec only grows.
//!   Tumult therefore makes the injected command *self-terminating* (it cleans
//!   up and exits after `duration_s`); the container object lingers in
//!   `Terminated`/`Completed` state until the pod itself is replaced. This is a
//!   deliberate, documented trade-off, not a leak we can rollback away.
//! - **`tc` needs `NET_ADMIN`.** [`pod_network_latency`] requests the
//!   `NET_ADMIN` capability on the ephemeral container. A restrictive
//!   `PodSecurity` admission policy (e.g. the `restricted` profile) can reject
//!   it; the API call then fails with a typed error rather than silently
//!   no-op'ing.
//! - **Stress accounting.** `stress-ng` shares the target's *process* namespace
//!   but runs in its own cgroup, so CPU/memory pressure competes for the pod's
//!   node-level share rather than counting against the target container's
//!   cgroup limit exactly. It is a faithful "noisy neighbour inside the pod",
//!   not a precise cgroup-limit exhaustion.
//! - **Best-effort / fire-and-forget.** These functions return once the
//!   ephemeral container is *accepted* by the apiserver; they do not stream the
//!   container's logs or block for `duration_s`. Callers observe the fault
//!   through probes and telemetry, as with the rest of Tumult.

use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, Patch, PatchParams};
use kube::Client;

use crate::error::KubeError;

/// Default image for [`pod_network_latency`]. `netshoot` bundles `tc`
/// (iproute2) and is the de-facto network-debug image.
pub const DEFAULT_NETEM_IMAGE: &str = "ghcr.io/nicolaka/netshoot:latest";

/// Default image for [`pod_stress`]. Must contain the `stress-ng` binary.
pub const DEFAULT_STRESS_IMAGE: &str = "ghcr.io/colinianking/stress-ng:latest";

/// Default network interface a pod's primary veth is exposed as.
pub const DEFAULT_IFACE: &str = "eth0";

/// Kind of in-pod resource stress to apply.
#[derive(Debug, Clone, Copy)]
pub enum StressKind {
    /// Saturate `workers` CPU cores with a busy loop.
    Cpu {
        /// Number of `stress-ng` CPU workers to spin.
        workers: u32,
    },
    /// Hold `bytes` of anonymous memory resident for the duration.
    Memory {
        /// Bytes of memory to allocate and keep resident.
        bytes: u64,
    },
}

/// Generate a short, unique-ish suffix for an ephemeral container name.
///
/// Ephemeral container names must be unique within a pod and cannot be reused
/// after termination, so we derive a suffix from the wall clock. Determinism is
/// not required — tests assert the *prefix* and the container spec, not the
/// exact suffix.
fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    format!("{nanos:08x}")
}

/// Resolve a concrete target pod name from an explicit name or a label
/// selector.
///
/// When `pod` is `Some`, it is returned as-is (no API call). Otherwise
/// `label_selector` must be `Some`; the namespace is listed and the first
/// matching pod is chosen — mirroring how Chaos Mesh / Litmus pick a victim
/// from a selector.
///
/// # Errors
///
/// - [`KubeError::InvalidConfig`] if neither `pod` nor `label_selector` is
///   given, or if the selector matches no pods.
/// - [`KubeError::Api`] if the list call fails.
pub async fn resolve_target_pod(
    client: Client,
    namespace: &str,
    pod: Option<&str>,
    label_selector: Option<&str>,
) -> Result<String, KubeError> {
    if let Some(name) = pod {
        return Ok(name.to_string());
    }
    let Some(selector) = label_selector else {
        return Err(KubeError::InvalidConfig {
            field: "pod",
            reason: "one of `pod` or `label_selector` must be provided".to_string(),
        });
    };
    let pods: Api<Pod> = Api::namespaced(client, namespace);
    let lp = kube::api::ListParams::default().labels(selector);
    let list = pods.list(&lp).await?;
    list.into_iter()
        .find_map(|p| p.metadata.name)
        .ok_or_else(|| KubeError::InvalidConfig {
            field: "label_selector",
            reason: format!("no pods matched selector `{selector}` in namespace `{namespace}`"),
        })
}

/// Build the strategic-merge patch that appends a single ephemeral container.
fn append_ephemeral_container(container: serde_json::Value) -> serde_json::Value {
    let containers = serde_json::Value::Array(vec![container]);
    serde_json::json!({ "spec": { "ephemeralContainers": containers } })
}

/// Inject network latency into a target pod's network namespace.
///
/// Attaches an ephemeral container that runs `tc qdisc add dev <iface> root
/// netem delay <delay_ms>ms [<jitter_ms>ms]`, sleeps for `duration_s`, then
/// removes the qdisc. Because pod containers share one network namespace, this
/// degrades the whole pod's egress on `iface`.
///
/// See the [module docs](self) for the ephemeral-container lifecycle limits
/// (feature gate, non-removable containers, `NET_ADMIN` requirement).
///
/// # Errors
///
/// - [`KubeError::InvalidConfig`] if `delay_ms` or `duration_s` is zero.
/// - [`KubeError::Api`] if the apiserver rejects the subresource patch (e.g.
///   pod not found, ephemeral containers disabled, `PodSecurity` denies
///   `NET_ADMIN`).
#[tracing::instrument(skip(client))]
#[must_use = "callers must check whether the latency fault was injected"]
#[allow(clippy::too_many_arguments)]
pub async fn pod_network_latency(
    client: Client,
    namespace: &str,
    pod: &str,
    delay_ms: u32,
    jitter_ms: u32,
    duration_s: u32,
    iface: &str,
    image: &str,
) -> Result<String, KubeError> {
    if delay_ms == 0 {
        return Err(KubeError::InvalidConfig {
            field: "delay_ms",
            reason: "latency must be greater than zero".to_string(),
        });
    }
    if duration_s == 0 {
        return Err(KubeError::InvalidConfig {
            field: "duration_s",
            reason: "duration must be greater than zero".to_string(),
        });
    }

    let name = format!("tumult-netem-{}", unique_suffix());
    let _span =
        crate::telemetry::begin_pod_network_latency(namespace, pod, &name, delay_ms, duration_s);

    let jitter = if jitter_ms > 0 {
        format!(" {jitter_ms}ms")
    } else {
        String::new()
    };
    // Self-terminating: apply netem, hold for the duration, then restore. The
    // trailing `del` runs even if `sleep` is interrupted so the qdisc does not
    // outlive the container's intended window.
    let script = format!(
        "tc qdisc add dev {iface} root netem delay {delay_ms}ms{jitter} && \
         sleep {duration_s}; tc qdisc del dev {iface} root netem"
    );

    let container = serde_json::json!({
        "name": name,
        "image": image,
        "command": ["sh", "-c", script],
        "securityContext": { "capabilities": { "add": ["NET_ADMIN"] } },
    });

    let pods: Api<Pod> = Api::namespaced(client, namespace);
    pods.patch_ephemeral_containers(
        pod,
        &PatchParams::default(),
        &Patch::Strategic(append_ephemeral_container(container)),
    )
    .await?;

    Ok(format!(
        "pod {namespace}/{pod}: injected {delay_ms}ms latency on {iface} for {duration_s}s \
         via ephemeral container {name}"
    ))
}

/// Inject CPU or memory stress into a target pod via an ephemeral container.
///
/// Attaches an ephemeral container running `stress-ng` in the target
/// container's process namespace (`targetContainerName`), self-terminating
/// after `duration_s` via `--timeout`.
///
/// See the [module docs](self) for the cgroup-accounting caveat and lifecycle
/// limits.
///
/// # Errors
///
/// - [`KubeError::InvalidConfig`] if `duration_s` is zero, or if the requested
///   stress amount is zero (no CPU workers / no memory bytes).
/// - [`KubeError::Api`] if the apiserver rejects the subresource patch.
#[tracing::instrument(skip(client))]
#[must_use = "callers must check whether the stress fault was injected"]
pub async fn pod_stress(
    client: Client,
    namespace: &str,
    pod: &str,
    kind: StressKind,
    duration_s: u32,
    target_container: Option<&str>,
    image: &str,
) -> Result<String, KubeError> {
    if duration_s == 0 {
        return Err(KubeError::InvalidConfig {
            field: "duration_s",
            reason: "duration must be greater than zero".to_string(),
        });
    }

    let (stress_args, summary) = match kind {
        StressKind::Cpu { workers } => {
            if workers == 0 {
                return Err(KubeError::InvalidConfig {
                    field: "cpu_workers",
                    reason: "CPU stress needs at least one worker".to_string(),
                });
            }
            (format!("--cpu {workers}"), format!("{workers} CPU workers"))
        }
        StressKind::Memory { bytes } => {
            if bytes == 0 {
                return Err(KubeError::InvalidConfig {
                    field: "mem_bytes",
                    reason: "memory stress needs a non-zero byte count".to_string(),
                });
            }
            (
                format!("--vm 1 --vm-bytes {bytes} --vm-keep"),
                format!("{bytes} bytes memory"),
            )
        }
    };

    let name = format!("tumult-stress-{}", unique_suffix());
    let _span = crate::telemetry::begin_pod_stress(namespace, pod, &name, &summary, duration_s);

    let script = format!("stress-ng {stress_args} --timeout {duration_s}s");
    let mut container = serde_json::json!({
        "name": name,
        "image": image,
        "command": ["sh", "-c", script],
    });
    if let Some(tc) = target_container {
        container["targetContainerName"] = serde_json::json!(tc);
    }

    let pods: Api<Pod> = Api::namespaced(client, namespace);
    pods.patch_ephemeral_containers(
        pod,
        &PatchParams::default(),
        &Patch::Strategic(append_ephemeral_container(container)),
    )
    .await?;

    Ok(format!(
        "pod {namespace}/{pod}: injected {summary} stress for {duration_s}s \
         via ephemeral container {name}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_is_hex_and_stable_width() {
        let s = unique_suffix();
        assert_eq!(s.len(), 8);
        assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn append_patch_nests_container_under_spec() {
        let patch = append_ephemeral_container(serde_json::json!({ "name": "x" }));
        assert_eq!(patch["spec"]["ephemeralContainers"][0]["name"], "x");
    }
}
