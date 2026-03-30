---
title: tumult-stress
parent: Plugins
nav_order: 8
---

# tumult-stress — CPU, Memory, and IO Stress

Script-based plugin for injecting resource stress via [stress-ng](https://github.com/ColinIanKing/stress-ng).

## Prerequisites

- `stress-ng` installed on the target host (`apt install stress-ng` / `brew install stress-ng`)

## Actions

| Action | Description | Key Variables |
|--------|-------------|---------------|
| `cpu-stress` | Inject CPU stress | `TUMULT_WORKERS`, `TUMULT_LOAD` (0-100), `TUMULT_TIMEOUT` |
| `memory-stress` | Inject memory pressure | `TUMULT_WORKERS`, `TUMULT_BYTES` (e.g., 256m), `TUMULT_TIMEOUT` |
| `io-stress` | Inject IO stress | `TUMULT_WORKERS`, `TUMULT_HDD_BYTES` (e.g., 1g), `TUMULT_TIMEOUT` |
| `combined-stress` | CPU + memory + IO combined | `TUMULT_CPU_WORKERS`, `TUMULT_CPU_LOAD`, `TUMULT_VM_WORKERS`, `TUMULT_VM_BYTES`, `TUMULT_HDD_WORKERS`, `TUMULT_TIMEOUT` |

## Probes

| Probe | Description | Output |
|-------|-------------|--------|
| `cpu-utilization` | Current CPU usage | Float 0-100 |
| `memory-utilization` | Current memory usage | Float 0-100 |
| `io-utilization` | Current IO wait | Float 0-100 |

Probes work on both Linux (`/proc/stat`, `/proc/meminfo`) and macOS (`top`, `vm_stat`, `iostat`).

## Example Experiment

```toon
method[1]:
  - name: stress-cpu
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-stress/actions/cpu-stress.sh
      env:
        TUMULT_WORKERS: 4
        TUMULT_LOAD: 80
        TUMULT_TIMEOUT: 60
    background: false

steady_state_hypothesis:
  title: CPU below 90%
  probes[1]:
    - name: check-cpu
      activity_type: probe
      provider:
        type: process
        path: plugins/tumult-stress/probes/cpu-utilization.sh
      tolerance:
        type: range
        from: 0.0
        to: 90.0
```
