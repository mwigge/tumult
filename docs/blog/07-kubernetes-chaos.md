---
title: "Kubernetes Chaos: Deep Fault Injection with tumult-kubernetes"
parent: Blog
nav_order: 7
---

# <img src="/images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> Kubernetes Chaos: Deep Fault Injection with tumult-kubernetes

![Tumult Banner](/images/tumult-banner.png)

*Part 7 of the Tumult series. [← Part 6: Data-Driven Chaos: Analytics Pipeline](./06-analytics-pipeline.md)*

---

Kubernetes has become the dominant platform for running production workloads, and chaos engineering for Kubernetes-native systems requires first-class Kubernetes API access. Shell scripts wrapping `kubectl` work up to a point, but they introduce fragility: dependency on the `kubectl` binary version, error handling through text parsing, and no access to the Kubernetes watch API for precise timing.

`tumult-kubernetes` is a native Rust plugin using `kube-rs` — a full async Kubernetes client — for deep, typed fault injection without the `kubectl` dependency.

---

## What `tumult-kubernetes` Can Do

| Capability | Action | Description |
|-----------|--------|-------------|
| **Pod chaos** | `delete_pod` | Immediate or graceful pod deletion |
| **Deployment chaos** | `scale_deployment` | Scale replicas to zero, down, or up |
| **Node chaos** | `cordon_node` | Mark node unschedulable |
| | `uncordon_node` | Restore node schedulability |
| | `drain_node` | Cordon + evict all non-DaemonSet pods |
| **Network chaos** | `apply_network_policy` | Create network isolation policies |
| | `delete_network_policy` | Remove network isolation |
| **Probes** | `pod_is_ready` | Is a specific pod ready? |
| | `all_pods_ready` | Are all pods matching a label selector ready? |
| | `deployment_is_ready` | Is a deployment fully available? |
| | `node_status` | Node conditions and schedulability |
| | `service_has_endpoints` | Does a service have healthy backends? |
| | `count_pods_in_phase` | Count pods in a specific phase |

---

## Authentication

`tumult-kubernetes` uses all `kube-rs` authentication methods:

```bash
# Use ~/.kube/config (default)
tumult run experiment.toon

# Specify a custom kubeconfig
KUBECONFIG=/path/to/cluster.yaml tumult run experiment.toon

# In-cluster (running inside Kubernetes)
# Automatically detected when KUBERNETES_SERVICE_HOST is set
tumult run experiment.toon
```

No `kubectl` required. The Kubernetes API calls happen directly from the Tumult binary using the cluster's service account or kubeconfig credentials.

---

## Scenario 1: Pod Deletion — The Most Common Kubernetes Chaos Test

Pod deletion is the "hello world" of Kubernetes chaos testing. Every Kubernetes workload should survive the deletion of individual pods — that is the entire premise of ReplicaSets and Deployments. But many teams discover edge cases only when they run the test: slow readiness probes, missing pod disruption budgets, sticky sessions that break on pod replacement.

```toon
title: API deployment survives pod deletion
description: |
  Delete an API pod and verify the deployment recovers within 30 seconds.
  Validates ReplicaSet behavior, readiness probe configuration, and
  load balancer endpoint updates.

tags[3]: kubernetes, pod-chaos, resilience

estimate:
  expected_outcome: recovered
  expected_recovery_s: 30.0
  expected_degradation: minor
  expected_data_loss: false
  confidence: high
  rationale: Deployment has 3 replicas; single pod loss should trigger immediate replacement
  prior_runs: 8

steady_state_hypothesis:
  title: All API pods ready and service endpoints populated
  probes[2]:
    - name: api-deployment-ready
      activity_type: probe
      provider:
        type: native
        plugin: tumult-kubernetes
        function: deployment_is_ready
        arguments:
          namespace: production
          name: api-server
      tolerance:
        type: exact
        value: true

    - name: api-service-has-endpoints
      activity_type: probe
      provider:
        type: native
        plugin: tumult-kubernetes
        function: service_has_endpoints
        arguments:
          namespace: production
          name: api-service
      tolerance:
        type: exact
        value: true

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
        grace_period_seconds: 0      # immediate kill, no graceful shutdown
    pause_after_s: 5.0

rollbacks[1]:
  - name: ensure-deployment-scaled
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

---

## Scenario 2: Deployment Scale-to-Zero — Validating Health Check Propagation

Scale-to-zero chaos tests a different failure mode: not a sudden pod death, but a graceful drain. This validates that your load balancer (or Kubernetes service endpoint controller) correctly removes the service from rotation as pods go down.

```toon
title: Payments service survives scale-to-zero and recovery
description: |
  Scale the payments deployment to zero replicas and verify traffic
  is correctly shed before scaling back up to validate full recovery.
  Tests endpoint propagation, circuit breaker behavior, and graceful
  upstream handling.

tags[3]: kubernetes, scale-chaos, payments

estimate:
  expected_outcome: deviated
  expected_recovery_s: 45.0
  expected_degradation: severe
  expected_data_loss: false
  confidence: medium
  rationale: Scale-to-zero will cause HTTP 503s during the window; recovery depends on Kubernetes scheduler and readiness probes
  prior_runs: 2

steady_state_hypothesis:
  title: Payments API responds successfully
  probes[1]:
    - name: payments-health
      activity_type: probe
      provider:
        type: http
        method: GET
        url: http://payments-service.production.svc.cluster.local/health
        timeout_s: 5.0
      tolerance:
        type: exact
        value: 200

method[2]:
  - name: scale-payments-to-zero
    activity_type: action
    provider:
      type: native
      plugin: tumult-kubernetes
      function: scale_deployment
      arguments:
        namespace: production
        name: payments-api
        replicas: 0
    pause_after_s: 10.0

  - name: check-pods-terminated
    activity_type: probe
    provider:
      type: native
      plugin: tumult-kubernetes
      function: count_pods_in_phase
      arguments:
        namespace: production
        label_selector: app=payments-api
        phase: Running
    tolerance:
      type: exact
      value: 0

rollbacks[1]:
  - name: restore-payments-replicas
    activity_type: action
    provider:
      type: native
      plugin: tumult-kubernetes
      function: scale_deployment
      arguments:
        namespace: production
        name: payments-api
        replicas: 3

regulatory:
  frameworks[1]: DORA
  requirements[1]:
    - id: DORA-Art25
      description: ICT resilience testing
      evidence: Recovery from complete service outage within declared RTO
```

---

## Scenario 3: Node Drain — Testing Cluster-Level Resilience

Node drain is a higher blast radius than pod deletion. Draining a node evicts all non-DaemonSet pods on that node, which may include pods from multiple deployments. This tests whether the cluster has sufficient capacity to accommodate all evicted workloads on remaining nodes.

```toon
title: Cluster survives node drain
description: |
  Drain one worker node and verify all workloads reschedule successfully.
  Tests node affinity rules, PodDisruptionBudgets, resource requests vs
  available capacity, and scheduling latency.

tags[3]: kubernetes, node-chaos, cluster

estimate:
  expected_outcome: recovered
  expected_recovery_s: 120.0
  expected_degradation: moderate
  expected_data_loss: false
  confidence: medium
  rationale: 3-node cluster; 1 node drain should be absorbed by remaining capacity
  prior_runs: 1

configuration:
  drain_target:
    type: env
    key: CHAOS_NODE_NAME

steady_state_hypothesis:
  title: All critical deployments healthy
  probes[3]:
    - name: api-ready
      activity_type: probe
      provider:
        type: native
        plugin: tumult-kubernetes
        function: all_pods_ready
        arguments:
          namespace: production
          label_selector: tier=api
      tolerance:
        type: exact
        value: true

    - name: worker-ready
      activity_type: probe
      provider:
        type: native
        plugin: tumult-kubernetes
        function: all_pods_ready
        arguments:
          namespace: production
          label_selector: tier=worker
      tolerance:
        type: exact
        value: true

    - name: db-ready
      activity_type: probe
      provider:
        type: native
        plugin: tumult-kubernetes
        function: all_pods_ready
        arguments:
          namespace: production
          label_selector: tier=database
      tolerance:
        type: exact
        value: true

method[1]:
  - name: drain-worker-node
    activity_type: action
    provider:
      type: native
      plugin: tumult-kubernetes
      function: drain_node
      arguments:
        name: "{{ configuration.drain_target }}"
        grace_period_seconds: 30
    pause_after_s: 30.0

rollbacks[1]:
  - name: uncordon-node
    activity_type: action
    provider:
      type: native
      plugin: tumult-kubernetes
      function: uncordon_node
      arguments:
        name: "{{ configuration.drain_target }}"
```

---

## Scenario 4: Network Policy — Simulating Network Partitions

Network chaos at the Kubernetes level uses NetworkPolicy resources to create selective network partitions between services. This tests whether your services degrade gracefully when a dependency becomes unreachable — rather than cascading failures.

```toon
title: Checkout service degrades gracefully when inventory unreachable
description: |
  Apply a NetworkPolicy that blocks traffic from checkout to inventory.
  Verify checkout falls back to cached inventory and continues processing
  orders without complete failure.

tags[3]: kubernetes, network-chaos, checkout

method[1]:
  - name: partition-inventory
    activity_type: action
    provider:
      type: native
      plugin: tumult-kubernetes
      function: apply_network_policy
      arguments:
        namespace: production
        policy:
          apiVersion: networking.k8s.io/v1
          kind: NetworkPolicy
          metadata:
            name: tumult-partition-inventory
          spec:
            podSelector:
              matchLabels:
                app: inventory-service
            ingress:
              - from:
                  - podSelector:
                      matchLabels:
                        app: NOT-checkout-service
    pause_after_s: 15.0

rollbacks[1]:
  - name: remove-partition
    activity_type: action
    provider:
      type: native
      plugin: tumult-kubernetes
      function: delete_network_policy
      arguments:
        namespace: production
        name: tumult-partition-inventory
```

---

## Label Selector Targeting

Rather than targeting specific pod names (which change on every deployment), `tumult-kubernetes` supports label selector targeting for most actions:

```toon
# Target any pod matching the label selector
- name: delete-api-pod-by-label
  activity_type: action
  provider:
    type: native
    plugin: tumult-kubernetes
    function: delete_pod
    arguments:
      namespace: production
      label_selector: app=api-server,version=v2
      # Deletes the first matching pod; use with care for multi-pod selections
```

This makes experiments stable across deployments. The experiment targets `app=api-server` — whatever pod has that label today.

---

## Running Against Multiple Environments

Tumult experiments parameterize through configuration, making the same experiment runnable against staging and production with different parameters:

```bash
# Run against staging
CHAOS_NAMESPACE=staging \
  CHAOS_NODE_NAME=staging-worker-02 \
  tumult run node-drain.toon

# Run against production (with approval gate in CI)
CHAOS_NAMESPACE=production \
  CHAOS_NODE_NAME=prod-worker-05 \
  tumult run node-drain.toon --rollback-strategy always
```

The `--rollback-strategy always` flag ensures rollbacks execute regardless of outcome — essential for production chaos experiments where leaving the system in a modified state is unacceptable.

---

## What to Watch in the Journal

After a Kubernetes chaos experiment, the journal contains:

```toon
status: completed
hypothesis_before_met: true
hypothesis_after_met: true

method_results[1]:
  - name: delete-api-pod
    status: succeeded
    duration_ms: 18
    output: "deleted pod api-server-7b8c9d-xk2p1"

hypothesis_after_results[2]:
  - name: api-deployment-ready
    status: succeeded
    duration_ms: 28734      # 28 seconds to full deployment recovery
    output: "true"
  - name: api-service-has-endpoints
    status: succeeded
    duration_ms: 31201
    output: "true"
```

The duration of the `hypothesis_after` probes tells you the actual recovery time — the time from the probe check starting until the deployment fully recovered. This is the real MTTR: not the time until the replacement pod was scheduled, but the time until it was ready to serve traffic.

**Update:** Kubernetes chaos is now fully validated on kind (K8s v1.35.0) with the native `tumult-kubernetes` plugin wired via kube-rs. Pod deletion, deployment scaling, readiness probes, and node cordon/uncordon all tested through `tumult run`. See the [test protocol](../testprotocol.md) for results.

---

*Try Tumult at [tumult.rs](https://tumult.rs) — or `curl -sSL https://raw.githubusercontent.com/mwigge/tumult/main/install.sh | sh`*

*Next in the series: [Part 8 — Statistical Baselines →](./08-statistical-baselines.md)*
