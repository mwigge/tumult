# Plugin Authoring Guide

Create community plugins for Tumult without a Rust toolchain. A plugin is a directory with a manifest and executable scripts.

## Directory Structure

```
tumult-nginx/
├── plugin.json          # manifest declaring actions and probes
├── actions/
│   ├── kill-worker.sh   # action: kill an nginx worker process
│   └── reload-config.sh # action: force config reload
├── probes/
│   └── connection-count.sh  # probe: count active connections
└── README.md            # optional documentation
```

## Manifest Format

The manifest file (`plugin.json`) declares the plugin's identity and capabilities:

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

See `docs/plugins/plugin-manifest-spec.md` for the full specification.

## Script Contract

Every script follows the same contract:

### Input

Arguments are passed as environment variables with the `TUMULT_` prefix. Keys are uppercased:

| Experiment argument | Environment variable |
|--------------------|--------------------|
| `worker_pid` | `TUMULT_WORKER_PID` |
| `signal` | `TUMULT_SIGNAL` |
| `max_wait` | `TUMULT_MAX_WAIT` |

### Output

| Channel | Purpose |
|---------|---------|
| `stdout` | Structured output (captured by engine) |
| `stderr` | Diagnostic messages (captured, logged) |

### Exit Code

| Code | Meaning |
|------|---------|
| `0` | Success |
| Non-zero | Failure |

### Example Action Script

```bash
#!/bin/bash
set -euo pipefail

# Receives: TUMULT_WORKER_PID or discovers one
WORKER_PID="${TUMULT_WORKER_PID:-$(pgrep -f 'nginx: worker' | head -1)}"

if [ -z "$WORKER_PID" ]; then
    echo "no nginx worker process found" >&2
    exit 1
fi

kill -9 "$WORKER_PID"
echo "killed nginx worker pid=$WORKER_PID"
```

### Example Probe Script

```bash
#!/bin/bash
set -euo pipefail

# Count active connections from nginx stub_status
CONNECTIONS=$(curl -s http://localhost:8080/nginx_status | grep 'Active connections' | awk '{print $3}')

if [ -z "$CONNECTIONS" ]; then
    echo "failed to read nginx status" >&2
    exit 1
fi

echo "$CONNECTIONS"
```

## Plugin Discovery

Tumult searches for plugins in this order:

1. `./plugins/` — local to the experiment directory
2. `~/.tumult/plugins/` — user-global
3. `TUMULT_PLUGIN_PATH` environment variable — colon-separated custom paths

First-found-wins: if two plugins share the same name, the one discovered first is used.

## Testing Locally

Place your plugin directory in `./plugins/` next to your experiment file:

```
my-project/
├── plugins/
│   └── tumult-nginx/
│       ├── plugin.json
│       ├── actions/
│       │   └── kill-worker.sh
│       └── probes/
│           └── connection-count.sh
└── experiment.toon
```

Verify discovery:

```bash
tumult discover --plugin tumult-nginx
```

Test a single action:

```bash
# Set env vars manually and run the script directly
TUMULT_WORKER_PID=12345 ./plugins/tumult-nginx/actions/kill-worker.sh
```

## Publishing

There is no central registry. Share your plugin by:

- Packaging the directory as a `.tar.gz` and distributing via your team's artifact repository
- Hosting in a git repository — users clone into their `~/.tumult/plugins/` directory
- Including in a container image alongside the `tumult` binary

## Best Practices

- Always use `set -euo pipefail` in bash scripts
- Write idempotent actions where possible (running twice produces the same result)
- Include a `README.md` describing what the plugin does and what arguments it accepts
- Test scripts independently before wrapping in a plugin manifest
- Use descriptive names: `tumult-<technology>` (e.g., `tumult-nginx`, `tumult-haproxy`)
