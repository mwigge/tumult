---
title: Plugins
nav_order: 3
has_children: true
---

# Plugins

Tumult supports two plugin models: **script plugins** (any language, no Rust required) and **native Rust plugins** (compiled into the binary).

## Plugin Discovery Order

Tumult searches for plugins in this order — first found wins:

1. `./plugins/` — project-local plugins
2. `~/.tumult/plugins/` — user-level plugins
3. `$TUMULT_PLUGIN_PATH` — custom path override
4. Compiled-in native plugins (always available)

## Script Plugins vs Native Plugins

| | Script Plugins | Native Rust Plugins |
|---|---|---|
| **Language** | Any (bash, Python, Go, …) | Rust |
| **Distribution** | Directory + `plugin.toon` manifest | Compiled into the binary |
| **Dispatch** | Script execution with env vars | `NativeExecutor` trait, registered in a `NativeExecutorRegistry` |
| **Arguments** | `TUMULT_<KEY>` env vars | Direct Rust function call |
| **Result** | stdout + exit code | `Result<Value, Error>` |
| **Examples** | stress, containers, process, db, kafka, network, pumba, loadtest | ssh, net, kubernetes |

Each native crate implements the `NativeExecutor` trait from `tumult-plugin`; the CLI is a pure composition root that registers the executors. Calling an unknown plugin or function fails with an error listing the available names.

## Bundled Plugins

| Plugin | Type | Capabilities |
|---|---|---|
| [tumult-ssh](tumult-ssh.md) | Native | Remote execution over SSH (Ed25519, RSA, ECDSA, agent auth, host-key verification with `verify` default) |
| [tumult-kubernetes](tumult-kubernetes.md) | Native | Pod delete, deployment scale, node cordon/drain, status probes |
| tumult-net | Native | Privilege-free userspace TCP chaos proxy — latency, bandwidth throttle, fragmentation, byte corruption, connection termination |
| [tumult-pumba](tumult-pumba.md) | Script | Container-scoped network chaos via Pumba — netem, iptables, kill/pause/stop, stress |
| [tumult-network](tumult-network.md) | Script | `tc netem` latency, packet loss, corruption, DNS block, host partition |
| [tumult-db](tumult-db.md) | Script | PostgreSQL, MySQL, Redis chaos: kill connections, lock tables, latency |
| [tumult-kafka](tumult-kafka.md) | Script | Broker kill, partition, latency; consumer lag and ISR probes |
| [tumult-stress](tumult-stress.md) | Script | CPU, memory, IO stress via `stress-ng` |
| [tumult-containers](tumult-containers.md) | Script | Docker/Podman kill, stop, pause, resource limits |
| [tumult-process](tumult-process.md) | Script | Kill, suspend, resume processes by PID/name/pattern |
| [tumult-loadtest](tumult-loadtest.md) | Script | k6 and JMeter load drivers with OTel correlation |

## Writing Your Own Plugin

See the [Plugin Authoring Guide](authoring-guide.md) and [Plugin Manifest Specification](plugin-manifest-spec.md).
