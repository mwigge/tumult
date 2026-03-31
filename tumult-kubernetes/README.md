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

## More Information

See the [main README](../README.md) for project overview and setup.
