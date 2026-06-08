---
title: tumult-pumba
parent: Plugins
nav_order: 12
---

# tumult-pumba — Container-Scoped Network & Resource Chaos

Script-based plugin that drives [Pumba](https://github.com/alexei-led/pumba) for
cross-platform, container-scoped chaos. Unlike `tumult-network` (which uses host
`tc`/`iptables` and needs `CAP_NET_ADMIN` on the host), Pumba runs as a sidecar
container against the Docker socket, so it works on Linux, macOS, and Docker
Desktop without host-level network capabilities.

## Prerequisites

- **Docker** (or Podman) with access to the Docker socket
  (`/var/run/docker.sock`).
- The `ghcr.io/alexei-led/pumba:latest` image (pulled on first use).
- A running **target container** to disrupt.

## Actions

| Action | Description |
|--------|-------------|
| `netem-delay` | Add network latency to container egress traffic |
| `netem-loss` | Add packet loss to container egress traffic |
| `netem-duplicate` | Duplicate packets on container egress |
| `netem-corrupt` | Corrupt packets on container egress |
| `netem-rate` | Limit bandwidth on container egress |
| `iptables-loss` | Drop incoming packets to the container via iptables |
| `kill-container` | Kill the container with a configurable signal and timing |
| `pause-container` | Pause container processes for a duration |
| `stop-container` | Stop the container with a grace period and restart |
| `stress-container` | CPU / memory / IO stress injection inside the container |

## Probes

| Probe | Description |
|-------|-------------|
| `container-running` | Check whether the target container is running |
| `container-latency` | Measure network latency from inside the container |
| `container-packet-stats` | Read container network-interface packet counters |

## Example

`examples/pumba-latency.toon` injects egress latency into a running Redis
container and verifies it recovers. With the docker-compose lab up:

```bash
docker compose -p docker -f docker/docker-compose.yml up -d redis
tumult run examples/pumba-latency.toon
```

Pumba actions are short-lived sidecar containers — they apply the fault for a
bounded duration and then clean up automatically, so the disruption is
self-limiting even if the experiment is interrupted.

## Pumba vs `tumult-network`

| | `tumult-pumba` | `tumult-network` |
|--|----------------|------------------|
| Scope | A single container | Host interface (`lo`, `eth0`, …) |
| Mechanism | Pumba sidecar via Docker socket | Host `tc` / `iptables` / `/etc/hosts` |
| Capabilities | Docker socket access | `CAP_NET_ADMIN` / root on the host |
| Best for | Container labs, macOS/Docker Desktop | Bare-metal / VM hosts, DNS chaos |

See the [Network Chaos guide](tumult-network.md) for the host-level alternative.
