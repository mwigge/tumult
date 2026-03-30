---
title: "ADR-007: Kubernetes Native"
parent: Architecture Decisions
nav_order: 7
---

# ADR-007: Native Rust Plugin for Kubernetes

## Status

Accepted

## Context

Kubernetes chaos requires API-level operations (pod deletion, deployment scaling, node cordoning) that go beyond what shell scripts can achieve. We need to decide between a script plugin (kubectl wrapper) and a native Rust plugin (kube-rs).

## Decision

Use a **native Rust plugin** via `kube-rs` (v3.1+) and `k8s-openapi`.

### Rationale

1. **No kubectl dependency**: Script plugins would require `kubectl` installed and configured on the Tumult host. A native plugin uses the Kubernetes API directly.

2. **Label selector targeting**: Following LitmusChaos and Chaos Mesh patterns, we target resources by label selectors, not just by name. This requires the list/watch API which is ergonomic in kube-rs.

3. **Structured responses**: Native probes return typed `PodStatus`, `DeploymentStatus`, `NodeStatus` structs — not parsed kubectl text output.

4. **Auth flexibility**: kube-rs supports kubeconfig, in-cluster service account, and OIDC auth natively.

5. **Async operations**: Node drain requires cordon + list pods + delete each — this is naturally expressed in async Rust with kube-rs.

### Trade-offs

- Binary size increases (~2-3MB for kube-rs + TLS)
- Compilation time increases significantly
- K8s API version must be pinned (currently v1.32 via k8s-openapi)

## Comparison with Other Tools

| Tool | K8s Approach |
|------|-------------|
| LitmusChaos | CRD-based (ChaosEngine), Go operator |
| Chaos Mesh | CRD-based (PodChaos, NetworkChaos), Go operator |
| Gremlin | Agent-based, SaaS control plane |
| Chaos Toolkit | kubectl wrapper scripts (chaostoolkit-kubernetes) |
| **Tumult** | Native kube-rs, library crate, label-selector targeting |

## Consequences

- Tumult K8s support works without kubectl installed
- Label selectors enable targeting by deployment, team, or environment labels
- Probe results are structured JSON, suitable for TOON journal and analytics
- K8s version compatibility managed via k8s-openapi feature flags
