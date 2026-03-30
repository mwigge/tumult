---
title: tumult-process
parent: Plugins
nav_order: 10
---

# tumult-process — Process Chaos

Script-based plugin for process kill, suspend, and resume operations.

## Actions

| Action | Description | Key Variables |
|--------|-------------|---------------|
| `kill-process` | Kill by PID, name, or pattern | `TUMULT_PID`, `TUMULT_NAME`, `TUMULT_PATTERN`, `TUMULT_SIGNAL` |
| `suspend-process` | Freeze (SIGSTOP) | `TUMULT_PID`, `TUMULT_NAME`, `TUMULT_PATTERN` |
| `resume-process` | Resume (SIGCONT) | `TUMULT_PID`, `TUMULT_NAME`, `TUMULT_PATTERN` |

Target resolution priority: `TUMULT_PID` > `TUMULT_NAME` > `TUMULT_PATTERN` (first match wins).

## Probes

| Probe | Description | Output |
|-------|-------------|--------|
| `process-exists` | Is the process running? | `true` or `false` |
| `process-resources` | CPU and memory usage | JSON: `{"cpu_percent": N, "mem_percent": N, "running": bool}` |

## Example Experiment

```toon
steady_state_hypothesis:
  title: API process is running
  probes[1]:
    - name: api-alive
      activity_type: probe
      provider:
        type: process
        path: plugins/tumult-process/probes/process-exists.sh
        env:
          TUMULT_NAME: api-server
      tolerance:
        type: exact
        value: true

method[1]:
  - name: kill-api
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-process/actions/kill-process.sh
      env:
        TUMULT_NAME: api-server
        TUMULT_SIGNAL: TERM
    pause_after_s: 5.0

rollbacks[1]:
  - name: restart-api
    activity_type: action
    provider:
      type: process
      path: systemctl
      arguments[2]: start, api-server
```

## Works via SSH

All process plugin scripts work both locally and on remote hosts via `ExecutionTarget::Ssh`. The scripts use standard POSIX commands (`kill`, `pkill`, `pgrep`, `ps`) available on all Linux/macOS systems.
