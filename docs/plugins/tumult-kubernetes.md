---
title: tumult-kubernetes
parent: Plugins
nav_order: 4
---

# tumult-kubernetes — Kubernetes Chaos

Native Rust plugin using `kube-rs` for Kubernetes chaos engineering. Follows patterns from LitmusChaos, Chaos Mesh, and Chaos Toolkit.

## Features

- **Pod deletion** — immediate (SIGKILL) or graceful with configurable grace period
- **Deployment scaling** — scale to zero, scale down, or scale up
- **Node drain** — cordon + evict all non-DaemonSet pods
- **Network policy** — apply/remove network policies to simulate partition
- **Label selector targeting** — target pods by labels (like Chaos Mesh / LitmusChaos)
- **Structured probes** — typed status responses as JSON for journal analytics

## Authentication

Supports all `kube-rs` auth methods:
- **Kubeconfig** — `~/.kube/config` or `KUBECONFIG` env var
- **In-cluster** — service account token (for running inside K8s)
- **OIDC** — identity provider tokens

## Actions

| Action | Description | Parameters |
|--------|-------------|------------|
| `delete_pod` | Delete a specific pod | `namespace`, `name`, `grace_period_seconds` |
| `scale_deployment` | Scale replicas | `namespace`, `name`, `replicas` |
| `cordon_node` | Mark node unschedulable | `name` |
| `uncordon_node` | Mark node schedulable | `name` |
| `drain_node` | Cordon + evict pods | `name`, `grace_period_seconds` |
| `apply_network_policy` | Create a network policy | `namespace`, `policy` (NetworkPolicy object) |
| `delete_network_policy` | Remove a network policy | `namespace`, `name` |

## Probes

| Probe | Description | Returns |
|-------|-------------|---------|
| `pod_is_ready` | Is a specific pod ready? | `bool` |
| `pods_by_label` | List pods matching labels | `Vec<PodStatus>` |
| `all_pods_ready` | Are all matching pods ready? | `(total, ready)` |
| `deployment_is_ready` | Deployment replica status | `DeploymentStatus` |
| `node_status` | Node conditions + schedulability | `NodeStatus` |
| `service_has_endpoints` | Does service have backends? | `bool` |
| `count_pods_in_phase` | Count pods in specific phase | `usize` |

## Example Experiment

```toon
title: API deployment survives pod deletion
description: Delete API pods and verify recovery within 30s

tags[2]: kubernetes, resilience

steady_state_hypothesis:
  title: API deployment is healthy
  probes[1]:
    - name: api-pods-ready
      activity_type: probe
      provider:
        type: native
        plugin: tumult-kubernetes
        function: all_pods_ready
        arguments:
          namespace: production
          label_selector: app=api-server

method[1]:
  - name: delete-api-pod
    activity_type: action
    provider:
      type: native
      plugin: tumult-kubernetes
      function: delete_pod
      arguments:
        namespace: production
        name: api-server-7b8c9d-xk2p1
        grace_period_seconds: 0

rollbacks[1]:
  - name: uncordon-if-needed
    activity_type: action
    provider:
      type: native
      plugin: tumult-kubernetes
      function: scale_deployment
      arguments:
        namespace: production
        name: api-server
        replicas: 3
```

## Implementation Notes

- Uses `kube-rs` 3.1 — async-native, no kubectl dependency
- K8s API version: v1.32 (via `k8s-openapi` feature flag)
- See ADR-007 for the rationale behind native vs script approach
