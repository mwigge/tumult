# tumult-containers — Container Chaos

Script-based plugin for Docker and Podman container chaos.

## Prerequisites

- `docker` or `podman` installed and accessible

## Actions

| Action | Description | Key Variables |
|--------|-------------|---------------|
| `kill-container` | Kill a container (SIGKILL) | `TUMULT_CONTAINER_ID` (required), `TUMULT_SIGNAL`, `TUMULT_RUNTIME` |
| `stop-container` | Graceful stop | `TUMULT_CONTAINER_ID` (required), `TUMULT_TIMEOUT`, `TUMULT_RUNTIME` |
| `pause-container` | Freeze all processes | `TUMULT_CONTAINER_ID` (required), `TUMULT_RUNTIME` |
| `unpause-container` | Resume frozen container | `TUMULT_CONTAINER_ID` (required), `TUMULT_RUNTIME` |

## Probes

| Probe | Description | Output |
|-------|-------------|--------|
| `container-status` | Container state | `running`, `stopped`, `paused`, `not_found` |
| `container-health` | Health check status | `healthy`, `unhealthy`, `starting`, `none`, `not_found` |

## Runtime Selection

Set `TUMULT_RUNTIME` to `docker` (default) or `podman`:

```toon
method[1]:
  - name: kill-db-container
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-containers/actions/kill-container.sh
      env:
        TUMULT_CONTAINER_ID: postgres-primary
        TUMULT_RUNTIME: podman
```

## Example Experiment

```toon
steady_state_hypothesis:
  title: API container is healthy
  probes[1]:
    - name: check-api
      activity_type: probe
      provider:
        type: process
        path: plugins/tumult-containers/probes/container-health.sh
        env:
          TUMULT_CONTAINER_ID: api-server
      tolerance:
        type: exact
        value: healthy

method[1]:
  - name: kill-api
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-containers/actions/kill-container.sh
      env:
        TUMULT_CONTAINER_ID: api-server

rollbacks[1]:
  - name: restart-api
    activity_type: action
    provider:
      type: process
      path: docker
      arguments[2]: start, api-server
```
