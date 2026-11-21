---
title: tumult-process
parent: Plugins
nav_order: 10
---

# tumult-process

Kill, freeze or resume a process on the machine that runs Tumult. The
scripts use `kill`, `pkill`, `pgrep` and `ps`, so they work on Linux and
macOS.

Each function finds its process by one of three arguments: `pid`, `name`
or `pattern`. When you give more than one, the first in this list wins:

```mermaid
flowchart LR
    accTitle: How tumult-process finds the process
    accDescr: The script uses pid when it is set. Otherwise it uses name, a match on the process name. Otherwise it uses pattern, a match on the whole command line.
    pid[pid: one process ID] -->|not set| name[name: a match on the process name]
    name -->|not set| pattern[pattern: a match on the command line]
```

## Functions

| Function | Kind | What it does | More arguments |
|---|---|---|---|
| `kill-process` | action | Sends a signal | `signal` (`KILL`) |
| `suspend-process` | action | Freezes the process (`SIGSTOP`) | |
| `resume-process` | action | Lets it run again (`SIGCONT`) | |
| `process-exists` | probe | `true` or `false` | |
| `process-resources` | probe | JSON: `cpu_percent`, `mem_percent`, `running` | |

## Example

```toon
title: The API restarts after a kill

steady_state_hypothesis:
  title: The API process runs
  probes[1]:
    - name: api-alive
      activity_type: probe
      provider:
        type: script
        plugin: tumult-process
        function: process-exists
        arguments:
          name: api-server
      tolerance:
        type: exact
        value: true

method[1]:
  - name: stop-the-api
    activity_type: action
    provider:
      type: script
      plugin: tumult-process
      function: kill-process
      arguments:
        name: api-server
        signal: TERM
    pause_after_s: 5.0
```

The check after the method passes only when something, such as systemd,
starts the process again within 5 seconds. To act on a process on another
host, run the script there with [tumult-ssh](tumult-ssh.md).
