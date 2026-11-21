---
title: tumult-stress
parent: Plugins
nav_order: 8
---

# tumult-stress

Load the CPU, the memory or the disk of the machine that runs Tumult with
[stress-ng](https://github.com/ColinIanKing/stress-ng), and read how busy
it is. Install `stress-ng` first (`apt install stress-ng` or
`brew install stress-ng`).

```mermaid
flowchart LR
    accTitle: A stress step and its probes
    accDescr: The stress action runs stress-ng for a set time and returns when it ends. The utilization probes read the load of the machine before and after.
    before[Probe: CPU in use] --> stress[cpu-stress runs stress-ng for timeout seconds] --> after[Probe: CPU in use again]
```

## Faults

Each action runs `stress-ng` for `timeout` seconds (30) and returns when it
ends.

| Action | What it loads | Arguments |
|---|---|---|
| `cpu-stress` | CPU | `workers` (0 means one per CPU), `load` in percent (100) |
| `memory-stress` | Memory | `workers` (1), `bytes` per worker (`256m`) |
| `io-stress` | Disk | `workers` (1), `hdd_bytes` (`1g`) |
| `combined-stress` | All three | `cpu_workers` (2), `cpu_load` (80), `vm_workers` (1), `vm_bytes` (`256m`), `hdd_workers` (1) |

## Probes

| Probe | Result |
|---|---|
| `cpu-utilization` | CPU in use, 0 to 100 |
| `memory-utilization` | Memory in use, 0 to 100 |
| `io-utilization` | Time waiting for IO, 0 to 100 |

The probes read `/proc` on Linux, and `top`, `vm_stat` and `iostat` on
macOS.

## Example

```toon
title: The host recovers from CPU load

steady_state_hypothesis:
  title: CPU below 90 percent
  probes[1]:
    - name: cpu-in-use
      activity_type: probe
      provider:
        type: script
        plugin: tumult-stress
        function: cpu-utilization
      tolerance:
        type: range
        from: 0.0
        to: 90.0

method[1]:
  - name: load-the-cpu
    activity_type: action
    provider:
      type: script
      plugin: tumult-stress
      function: cpu-stress
      arguments:
        workers: 4
        load: 80
        timeout: 60
      timeout_s: 90.0
```

To load another host, run `stress-ng` there with
[tumult-ssh](tumult-ssh.md). To load one container, use
`stress-container` of [tumult-pumba](tumult-pumba.md).
