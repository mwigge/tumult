---
title: tumult-network
parent: Plugins
nav_order: 5
---

# tumult-network — Network Chaos

Script-based plugin for network fault injection. Covers latency, packet loss, corruption, DNS disruption (blocking, delay, redirect), and host partitioning.

## Prerequisites

- **Linux** with `tc` (iproute2) and `iptables` for network-level actions
- **Root/sudo** access for tc netem and iptables rules
- DNS redirect and targeted DNS block use `/etc/hosts` — works on Linux, macOS, and containers without `CAP_NET_ADMIN`
- Probes (ping, DNS) work on both Linux and macOS

## Actions

| Action | Description | Key Variables |
|--------|-------------|---------------|
| `add-latency` | Add network latency (tc netem) | `TUMULT_INTERFACE`, `TUMULT_DELAY_MS`, `TUMULT_JITTER_MS`, `TUMULT_TARGET_IP` |
| `add-packet-loss` | Add packet loss (tc netem) | `TUMULT_INTERFACE`, `TUMULT_LOSS_PCT`, `TUMULT_CORRELATION` |
| `add-corruption` | Add packet corruption | `TUMULT_INTERFACE`, `TUMULT_CORRUPTION_PCT` |
| `reset-tc` | Remove all tc rules (rollback) | `TUMULT_INTERFACE` |
| `block-dns` | Block DNS — all traffic via iptables, or targeted domain via `/etc/hosts` | `TUMULT_DNS_PORT`, `TUMULT_DNS_DOMAIN` (optional) |
| `delay-dns` | Add latency to DNS queries (tc netem on port 53) | `TUMULT_INTERFACE`, `TUMULT_DNS_DELAY_MS`, `TUMULT_DNS_JITTER_MS` |
| `redirect-dns` | Redirect a domain to a wrong IP via `/etc/hosts` | `TUMULT_DNS_DOMAIN` (required), `TUMULT_DNS_REDIRECT` |
| `block-dns-rollback` | Remove DNS block entries from `/etc/hosts` and iptables | — |
| `partition-host` | Network partition via iptables DROP | `TUMULT_TARGET_IP` (required), `TUMULT_DIRECTION` |

## Probes

| Probe | Description | Output |
|-------|-------------|--------|
| `ping-latency` | Round-trip latency (ms) | Float |
| `dns-resolve` | DNS resolution check | IP address or `"failed"` |
| `dns-latency` | DNS resolution time (ms) | Integer ms or `"error"` |

## DNS Chaos

### Targeted DNS blocking

Block a specific domain without affecting all DNS traffic:

```bash
TUMULT_DNS_DOMAIN=api.example.com tumult run block-dns-experiment.toon
```

Uses `/etc/hosts` — portable, works in containers, no `CAP_NET_ADMIN` needed.

### DNS latency injection

Slow down all DNS queries without blocking them:

```bash
TUMULT_DNS_DELAY_MS=500 TUMULT_DNS_JITTER_MS=50 tumult run delay-dns-experiment.toon
```

Uses `tc netem` on port 53 — works on DNS wire format regardless of encoding.

### DNS redirect

Point a domain to a wrong IP to test failover/retry behavior:

```bash
TUMULT_DNS_DOMAIN=upstream.service TUMULT_DNS_REDIRECT=127.0.0.1 tumult run redirect-dns-experiment.toon
```

### Design note

DNS actions use `/etc/hosts` and `tc netem` instead of `iptables --string` matching. DNS wire format encodes domains as length-prefixed labels (`\x06google\x03com\x00`), not dot-separated strings. `iptables --string "google.com"` will not match DNS packets.

## Rollback

For tc netem rules (latency, delay-dns):
```toon
rollbacks[1]:
  - name: cleanup
    activity_type: action
    provider:
      type: script
      plugin: tumult-network
      action: reset-tc
```

For DNS block/redirect (`/etc/hosts` + iptables):
```toon
rollbacks[1]:
  - name: cleanup-dns
    activity_type: action
    provider:
      type: script
      plugin: tumult-network
      action: block-dns-rollback
```
