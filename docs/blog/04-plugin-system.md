# <img src="../images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> The Plugin System: Chaos Engineering Without a Rust Toolchain

![Tumult Banner](../images/tumult-banner.png)

*Part 4 of the Tumult series. [← Part 3: Built-In Proof: Native Observability](./03-built-in-observability.md)*

---

Extensibility is where chaos engineering platforms have historically been most limiting. You want to inject a fault specific to your infrastructure — say, a Kafka broker partition or a Redis cache flush — and you find yourself either writing a complex Python extension, relying on a community plugin that is two years out of date, or resorting to shell scripts bolted to the side of your experiment definition.

Tumult approaches this differently. The plugin system has two layers: **script plugins** that anyone can write, and **native Rust plugins** that integrate deeply with SDKs. You choose the right layer for your use case, and the engine handles the rest.

---

## Two Layers, One Interface

From the experiment definition's perspective, a script plugin and a native plugin look identical:

```toon
# Using a native Rust plugin (tumult-kubernetes)
- name: delete-api-pod
  activity_type: action
  provider:
    type: native
    plugin: tumult-kubernetes
    function: delete_pod
    arguments:
      namespace: production
      name: api-server-7b8c9d-xk2p1

# Using a script plugin (tumult-network)
- name: inject-latency
  activity_type: action
  provider:
    type: process
    path: plugins/tumult-network/actions/add-latency.sh
    env:
      TUMULT_INTERFACE: eth0
      TUMULT_DELAY_MS: "200"
      TUMULT_JITTER_MS: "20"
```

The experiment author does not need to know whether the plugin is compiled Rust or a bash script. The engine executes both, captures stdout/stderr, records timing, and emits OTel spans with identical structure. The journal entry for a script action and a native action are indistinguishable.

---

## Script Plugins: The Community Layer

Script plugins are the primary path for community contributions and team-specific extensions. The contract is simple: a directory with a manifest and executable scripts.

### Directory structure

```
tumult-nginx/
├── plugin.toon              # manifest: declares actions and probes
├── actions/
│   ├── kill-worker.sh       # action: kill an nginx worker process
│   └── reload-config.sh     # action: force config reload
├── probes/
│   └── connection-count.sh  # probe: count active connections
└── README.md
```

### The manifest

The `plugin.toon` manifest declares what the plugin can do:

```json
{
  "name": "tumult-nginx",
  "version": "0.1.0",
  "description": "Nginx chaos actions and probes",
  "actions": [
    {
      "name": "kill-worker",
      "script": "actions/kill-worker.sh",
      "description": "Kill an nginx worker process"
    },
    {
      "name": "reload-config",
      "script": "actions/reload-config.sh",
      "description": "Force nginx config reload"
    }
  ],
  "probes": [
    {
      "name": "connection-count",
      "script": "probes/connection-count.sh",
      "description": "Count active nginx connections"
    }
  ]
}
```

### The script contract

Scripts communicate with the engine through a simple, consistent interface:

**Input**: Arguments are passed as environment variables with the `TUMULT_` prefix, uppercased. An argument `worker_pid` in the experiment becomes `TUMULT_WORKER_PID` in the script.

**Output**: 
- `stdout` — the result (captured by the engine, stored in the journal)
- `stderr` — diagnostic messages (captured, logged, not stored as result)
- Exit code `0` = success, non-zero = failure

**Example action script** (kill an nginx worker):

```bash
#!/bin/bash
set -euo pipefail

WORKER_PID="${TUMULT_WORKER_PID:-$(pgrep -f 'nginx: worker' | head -1)}"

if [ -z "$WORKER_PID" ]; then
    echo "no nginx worker process found" >&2
    exit 1
fi

kill -9 "$WORKER_PID"
echo "killed nginx worker pid=$WORKER_PID"
```

**Example probe script** (count nginx connections):

```bash
#!/bin/bash
set -euo pipefail

CONNECTIONS=$(curl -s http://localhost:8080/nginx_status \
  | grep 'Active connections' \
  | awk '{print $3}')

if [ -z "$CONNECTIONS" ]; then
    echo "failed to read nginx status" >&2
    exit 1
fi

echo "$CONNECTIONS"
```

The probe outputs a number. The experiment's tolerance definition determines whether that number represents a healthy state:

```toon
- name: check-connections
  activity_type: probe
  provider:
    type: process
    path: plugins/tumult-nginx/probes/connection-count.sh
  tolerance:
    type: range
    from: 0
    to: 1000
```

---

## What Script Plugins Enable

The script approach means that any infrastructure capability expressible in a shell script is a Tumult plugin. Consider what that covers:

**Existing plugins in the ecosystem:**

| Plugin | Fault | Probe |
|--------|-------|-------|
| `tumult-stress` | `stress-ng` CPU/memory/IO stress | CPU utilization, memory pressure |
| `tumult-containers` | Docker/Podman kill, stop, pause, resource limits | Container health, restart count |
| `tumult-process` | Kill/suspend/resume process by PID or name | Process status, CPU usage |
| `tumult-network` | tc netem latency/loss/corruption, DNS block, iptables partition | Ping latency, DNS resolution |
| `tumult-db-postgres` | Kill connections, lock tables, exhaust connection pool | Active connections, query latency |
| `tumult-db-redis` | FLUSHALL, CLIENT PAUSE, DEBUG SLEEP | Memory usage, connected clients |
| `tumult-kafka` | Kill broker, add partition latency | Consumer lag, broker availability |

**And anything you can write:**
- HAProxy backend removal
- Vault secret revocation
- S3 bucket policy modification
- Custom health endpoint failures
- Feature flag manipulation via your internal API

If you can express it in a script, it is a Tumult plugin.

---

## A Complete Script Plugin Example: HAProxy Backend Removal

Here is a realistic custom plugin that removes a backend server from HAProxy's active pool — simulating a graceful backend removal — and verifies the remaining backends are healthy.

**Directory structure:**
```
tumult-haproxy/
├── plugin.toon
├── actions/
│   ├── disable-backend.sh
│   └── enable-backend.sh
└── probes/
    └── active-backends.sh
```

**`plugin.toon`:**
```json
{
  "name": "tumult-haproxy",
  "version": "0.1.0",
  "description": "HAProxy chaos actions and probes",
  "actions": [
    {
      "name": "disable-backend",
      "script": "actions/disable-backend.sh",
      "description": "Disable a backend server in HAProxy"
    },
    {
      "name": "enable-backend",
      "script": "actions/enable-backend.sh",
      "description": "Re-enable a backend server in HAProxy"
    }
  ],
  "probes": [
    {
      "name": "active-backends",
      "script": "probes/active-backends.sh",
      "description": "Count active backends in a HAProxy backend pool"
    }
  ]
}
```

**`actions/disable-backend.sh`:**
```bash
#!/bin/bash
set -euo pipefail

BACKEND="${TUMULT_BACKEND:?TUMULT_BACKEND is required}"
SERVER="${TUMULT_SERVER:?TUMULT_SERVER is required}"
SOCKET="${TUMULT_SOCKET:-/run/haproxy/admin.sock}"

echo "set server ${BACKEND}/${SERVER} state maint" \
  | socat stdio "${SOCKET}"

echo "disabled ${BACKEND}/${SERVER}"
```

**`probes/active-backends.sh`:**
```bash
#!/bin/bash
set -euo pipefail

BACKEND="${TUMULT_BACKEND:?TUMULT_BACKEND is required}"
SOCKET="${TUMULT_SOCKET:-/run/haproxy/admin.sock}"

COUNT=$(echo "show servers state ${BACKEND}" \
  | socat stdio "${SOCKET}" \
  | awk 'NR>2 && $6==2 {count++} END {print count+0}')

echo "$COUNT"
```

**Experiment using the plugin:**
```toon
title: HAProxy backend removal maintains service
description: Remove one backend and verify requests continue

tags[2]: haproxy, resilience

steady_state_hypothesis:
  title: All backends healthy
  probes[1]:
    - name: backends-available
      activity_type: probe
      provider:
        type: process
        path: plugins/tumult-haproxy/probes/active-backends.sh
        env:
          TUMULT_BACKEND: web-servers
      tolerance:
        type: range
        from: 2
        to: 10

method[1]:
  - name: disable-web-03
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-haproxy/actions/disable-backend.sh
      env:
        TUMULT_BACKEND: web-servers
        TUMULT_SERVER: web-03
    pause_after_s: 10.0

rollbacks[1]:
  - name: re-enable-web-03
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-haproxy/actions/enable-backend.sh
      env:
        TUMULT_BACKEND: web-servers
        TUMULT_SERVER: web-03
```

---

## Native Plugins: When You Need SDK Depth

Script plugins cover most use cases. But some integrations require deep SDK access that bash cannot easily provide: Kubernetes API calls with authentication, cloud provider SDK operations, database driver-level fault injection.

For these, Tumult supports native Rust plugins compiled as feature-flagged crates:

```bash
# Build with Kubernetes and SSH support
cargo install tumult --features kubernetes,ssh

# Build with all native plugins
cargo install tumult --features kubernetes,ssh,analytics
```

The native plugin interface is the `ChaosPlugin` trait in `tumult-plugin`. A native plugin implements actions and probes as async Rust functions with typed arguments. The `tumult-kubernetes` plugin, for example, uses `kube-rs` — a full async Kubernetes client — for operations that would be difficult to express reliably in bash:

- Pod deletion with configurable grace periods
- Deployment scaling with wait-for-convergence
- Network policy application and removal
- Node drain with proper eviction handling

---

## Plugin Discovery

Tumult discovers plugins from three locations, in order:

```
1. ./plugins/           — local to the experiment directory
2. ~/.tumult/plugins/   — user-global (persistent across projects)
3. $TUMULT_PLUGIN_PATH  — colon-separated custom paths
```

First-found-wins. If the same plugin name appears in multiple paths, the first-discovered version is used.

Verify what is discovered:

```bash
# List all discovered plugins
tumult discover

# Show details for a specific plugin
tumult discover --plugin tumult-haproxy
```

Output:
```
tumult-haproxy v0.1.0
  HAProxy chaos actions and probes
  Path: ./plugins/tumult-haproxy

  Actions:
    disable-backend  Disable a backend server in HAProxy
    enable-backend   Re-enable a backend server in HAProxy

  Probes:
    active-backends  Count active backends in a HAProxy backend pool
```

---

## The No-Rust Requirement for Community Plugins

The design principle here is worth stating directly: **anyone who can write a bash script can write a Tumult plugin**. There is no Rust toolchain requirement, no build step, no compilation. You write scripts, write a manifest, and the plugin works.

This is the same philosophy that made Chaos Toolkit's Python extension model successful — lower the barrier to contribution and the ecosystem grows. But Tumult's script model goes further: because the engine is a single Rust binary with no runtime dependencies, plugins run in pristine environments without Python path conflicts or dependency collisions. The script executes, the engine captures the result, and the journal records it.

For platform teams building shared plugin libraries for their engineering organization, this model means plugins can live in a git repository alongside the experiments that use them, with no build infrastructure required for contributors.

---

*Next in the series: [Part 5 — Writing Your First Experiment: The TOON Format in Depth →](./05-experiment-format.md)*
