# tumult-network — Network Chaos

Script-based plugin for network fault injection. Covers latency, packet loss, corruption, DNS disruption, and host partitioning. Foundation for Kafka and database network chaos.

## Prerequisites

- **Linux** with `tc` (iproute2) and `iptables`
- **Root/sudo** access for tc netem and iptables rules
- Probes (ping, DNS) work on both Linux and macOS

## Actions

| Action | Description | Key Variables |
|--------|-------------|---------------|
| `add-latency` | Add network latency (tc netem) | `TUMULT_INTERFACE`, `TUMULT_DELAY_MS`, `TUMULT_JITTER_MS`, `TUMULT_TARGET_IP` |
| `add-packet-loss` | Add packet loss (tc netem) | `TUMULT_INTERFACE`, `TUMULT_LOSS_PCT`, `TUMULT_CORRELATION` |
| `add-corruption` | Add packet corruption | `TUMULT_INTERFACE`, `TUMULT_CORRUPTION_PCT` |
| `reset-tc` | Remove all tc rules (rollback) | `TUMULT_INTERFACE` |
| `block-dns` | Block DNS via iptables | `TUMULT_DNS_PORT` |
| `partition-host` | Network partition via iptables DROP | `TUMULT_TARGET_IP` (required), `TUMULT_DIRECTION` |

## Probes

| Probe | Description | Output |
|-------|-------------|--------|
| `ping-latency` | Round-trip latency (ms) | Float |
| `dns-resolve` | DNS resolution check | IP address or `"failed"` |

## Rollback

Always use `reset-tc` as a rollback action to clean up tc netem rules:

```toon
rollbacks[1]:
  - name: cleanup-latency
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-network/actions/reset-tc.sh
      env:
        TUMULT_INTERFACE: eth0
```

For iptables rules, use rollback scripts that remove rules by comment:
```sh
iptables -D OUTPUT -m comment --comment "tumult-partition" -j DROP
```
