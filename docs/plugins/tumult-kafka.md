---
title: tumult-kafka
parent: Plugins
nav_order: 7
---

# tumult-kafka — Kafka Broker Chaos

Script-based plugin for Apache Kafka chaos engineering. Covers broker failures, network partitions, latency injection, and consumer/cluster probes.

## Prerequisites

- Kafka CLI tools (`kafka-consumer-groups.sh`, `kafka-topics.sh`)
- For network chaos: `tc` (iproute2), `iptables` — Linux with root/sudo
- For broker kill: SSH access to broker hosts

## Actions

| Action | Description | Key Variables |
|--------|-------------|---------------|
| `kill-broker` | Kill Kafka broker process (SIGKILL) | `TUMULT_SIGNAL` |
| `partition-broker` | iptables partition between brokers | `TUMULT_BROKER_IP`, `TUMULT_CLUSTER_IPS` (comma-separated) |
| `add-broker-latency` | tc netem latency on Kafka port | `TUMULT_DELAY_MS`, `TUMULT_KAFKA_PORT` |

## Probes

| Probe | Description | Output |
|-------|-------------|--------|
| `consumer-lag` | Total consumer group lag | Integer (sum across partitions) |
| `under-replicated` | Under-replicated partition count | Integer (0 = healthy) |
| `broker-count` | Active brokers in cluster | Integer |

## Example: Broker Kill with Consumer Lag Monitoring

```toon
title: Kafka survives broker failure
description: Kill a broker and verify consumer lag recovers

steady_state_hypothesis:
  title: No under-replicated partitions
  probes[1]:
    - name: check-replicas
      activity_type: probe
      provider:
        type: process
        path: plugins/tumult-kafka/probes/under-replicated.sh
        env:
          TUMULT_KAFKA_BOOTSTRAP: broker-1:9092
      tolerance:
        type: exact
        value: 0

method[1]:
  - name: kill-broker-2
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-kafka/actions/kill-broker.sh
    execution_target:
      type: ssh
      host: broker-2
      user: kafka
      key_path: /home/ops/.ssh/id_ed25519

rollbacks[1]:
  - name: restart-broker
    activity_type: action
    provider:
      type: process
      path: /opt/kafka/bin/kafka-server-start.sh
      arguments[2]: -daemon, /opt/kafka/config/server.properties
    execution_target:
      type: ssh
      host: broker-2
      user: kafka
```

## Network Chaos for Kafka

The `tumult-network` plugin provides the foundation for Kafka network chaos. Use `add-broker-latency` for targeted latency on the Kafka port, or `partition-broker` for full network isolation between brokers.
