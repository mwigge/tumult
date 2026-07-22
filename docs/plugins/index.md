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
| **Examples** | containers, db-postgres, db-mysql, db-redis, kafka, loadtest, network, process, pumba, stress, timewarp | ssh, net, kubernetes, cloud, windows |

Each native crate implements the `NativeExecutor` trait from `tumult-plugin`; the CLI is a pure composition root that registers the executors. Calling an unknown plugin or function fails with an error listing the available names.

Script plugins are dispatched from experiments with the `script` provider —
`type: script` with `plugin:` and `function:` naming the plugin and action;
arguments are passed to the script as `TUMULT_*` environment variables:

```toon
provider:
  type: script
  plugin: tumult-db-postgres
  function: kill-connections
```

The bundled inventory is 16 plugins — 11 script + 5 native — with 91 actions
total (`tumult discover` prints the authoritative list for your build).

## Bundled Plugins

### Native executors (5)

| Plugin | Capabilities |
|---|---|
| [tumult-ssh](tumult-ssh.md) | Remote execution over SSH (Ed25519, RSA, ECDSA, agent auth, host-key verification with `verify` default) |
| tumult-net | Privilege-free userspace TCP chaos proxy — latency, bandwidth throttle, fragmentation, byte corruption, connection termination |
| [tumult-kubernetes](tumult-kubernetes.md) | Pod delete, deployment scale, node cordon/drain, status probes |
| tumult-cloud | AWS (EC2 stop/terminate, FIS experiments), Azure Chaos, GCP compute stop |
| tumult-windows | Native Windows faults: process-kill, CPU stress, firewall blackhole |

### Script plugins (11)

| Plugin | Capabilities |
|---|---|
| [tumult-containers](tumult-containers.md) | Docker/Podman kill, stop, pause, resource limits |
| [tumult-db](tumult-db.md) | tumult-db-postgres / tumult-db-mysql / tumult-db-redis: kill connections, lock tables, inject latency |
| [tumult-kafka](tumult-kafka.md) | Broker kill, partition, latency; consumer lag and ISR probes |
| [tumult-loadtest](tumult-loadtest.md) | k6 load driver with OTel correlation |
| [tumult-network](tumult-network.md) | `tc netem` latency, packet loss, corruption, DNS block, host partition |
| [tumult-process](tumult-process.md) | Kill, suspend, resume processes by PID/name/pattern |
| [tumult-pumba](tumult-pumba.md) | Container-scoped network chaos via Pumba — netem, iptables, kill/pause/stop, stress |
| [tumult-stress](tumult-stress.md) | CPU, memory, IO stress via `stress-ng` |
| tumult-timewarp | Clock-skew and entropy/crypto-skew faults via libfaketime — time drift, cert/token expiry, RNG starvation |

## Writing Your Own Plugin

See the [Plugin Authoring Guide](authoring-guide.md) and [Plugin Manifest Specification](plugin-manifest-spec.md).
