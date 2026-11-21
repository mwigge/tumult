# tumult-kubernetes

Kubernetes faults and probes for Tumult, through the API server with
`kube-rs`. `KubernetesExecutor` runs them as the `tumult-kubernetes`
native plugin. The functions, their arguments and the limits of the pod
faults are on the [plugin page](../docs/plugins/tumult-kubernetes.md).

## Use it from Rust

```rust
use tumult_kubernetes::actions::delete_pod;

let client = kube::Client::try_default().await?;
let message = delete_pod(client, "my-namespace", "my-pod", None).await?;
```

| Module | What it holds |
|---|---|
| `actions` | Faults: `delete_pod`, `scale_deployment`, `drain_node` and more |
| `probes` | Readings: `pod_is_ready`, `deployment_is_ready` and more |
| `inject` | The pod faults: an ephemeral container that runs `tc netem` or `stress-ng` in the target pod, then ends itself |
| `native` | The `KubernetesExecutor` and its function table |

## Test it

- Without a cluster: `cargo test -p tumult-kubernetes`. The harness in
  `tests/fake_apiserver.rs` answers as an API server and checks each
  request: the `ephemeralcontainers` patch, the container spec, the `tc`
  and `stress-ng` command, the label selector and the error paths.
- On a cluster: `scripts/k8s-demo.sh` starts a k3d cluster, runs
  `examples/k8s-pod-latency.toon` and `examples/k8s-pod-stress.toon` with
  the built binary and checks that each fault took effect. It needs `k3d`
  and `kubectl`.
