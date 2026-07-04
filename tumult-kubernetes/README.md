# tumult-kubernetes

Kubernetes chaos actions and probes for the Tumult platform -- pod disruption, network policies, and health probes.

## Key Types

- `K8sAction` -- chaos actions targeting Kubernetes resources
- `K8sProbe` -- health and readiness probes for Kubernetes workloads

## Usage

```rust
use tumult_kubernetes::K8sAction;

let action = K8sAction::delete_pod("my-namespace", "my-pod");
action.execute(&kube_client).await?;
```

## In-pod data-plane fault injection (ephemeral containers)

Chaos Mesh and LitmusChaos reach *inside* a pod (per-pod network latency,
in-pod CPU/memory stress) with a permanently-installed privileged `DaemonSet`.
Tumult closes that gap **without** a daemon — keeping its "no control plane,
single binary" identity — by using the Kubernetes *ephemeral containers*
subresource, the same mechanism `kubectl debug` uses. See the [`inject`] module
docs for full detail.

| Function | Mechanism | Key args |
| --- | --- | --- |
| `pod_network_latency` | Ephemeral container running `tc qdisc add … netem delay` in the pod's shared network namespace, self-terminating after `duration_s` (then `tc qdisc del`). Requests `NET_ADMIN`. | `namespace`, `pod` **or** `label_selector`, `delay_ms`, `jitter_ms`, `duration_s`, `iface`, `image` |
| `pod_stress` | Ephemeral container running `stress-ng` in the target container's process namespace (`targetContainerName`), self-terminating via `--timeout`. | `namespace`, `pod` **or** `label_selector`, `cpu_workers` **or** `mem_bytes`, `duration_s`, `target_container`, `image` |

### Limitations (vanilla cluster)

- **Feature gate.** Ephemeral containers are GA / on-by-default since Kubernetes
  1.25. On older clusters the subresource returns NotFound and injection fails
  with a typed error.
- **Non-removable.** Kubernetes forbids removing an ephemeral container once
  attached — the pod spec only grows. Tumult therefore injects a
  *self-terminating* command (clean up + exit after `duration_s`); the container
  object lingers in `Terminated` state until the pod is replaced. This is a
  documented trade-off, not a rollback we can perform.
- **`NET_ADMIN` / PodSecurity.** `tc` needs `NET_ADMIN`; a `restricted`
  PodSecurity profile can reject the ephemeral container. The API call then
  fails with a typed error rather than silently no-op'ing.
- **Stress accounting.** `stress-ng` shares the target's process namespace but
  runs in its own cgroup, so it competes for the pod's node-level share rather
  than exhausting the target container's cgroup limit exactly — a faithful
  in-pod "noisy neighbour", not precise limit exhaustion.
- **Fire-and-forget.** Both functions return once the apiserver *accepts* the
  ephemeral container; they do not stream logs or block for `duration_s`. Effect
  is observed via probes and telemetry.

### Validating

- **Hermetic (no cluster):** `cargo test -p tumult-kubernetes` — the
  `tests/fake_apiserver.rs` harness scripts a mock apiserver and asserts the
  exact `ephemeralcontainers` PATCH, container spec, `tc` / `stress-ng` command,
  label-selector resolution, and error paths.
- **Live cluster:** `scripts/k8s-demo.sh` spins up a k3d cluster, deploys a
  target pod, runs `examples/k8s-pod-latency.toon` and `examples/k8s-pod-stress.toon`
  through the built binary, and asserts the ephemeral containers attached and
  the fault took effect. Requires `k3d` + `kubectl`.

[`inject`]: ./src/inject.rs

## More Information

See the [main README](../README.md) for project overview and setup.
