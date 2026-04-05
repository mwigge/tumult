# Quickstart

Get Tumult running in 5 minutes.

## Install

### Option A: From source

```bash
curl -sSL https://raw.githubusercontent.com/mwigge/tumult/main/install.sh | sh
```

Builds the binary, starts Docker targets, runs a verification experiment. Requires [Rust](https://rustup.rs/) and Docker.

### Option B: Docker (no Rust toolchain needed)

```bash
docker pull ghcr.io/mwigge/tumult:latest        # CLI + MCP server
docker pull ghcr.io/mwigge/tumult-mcp:latest     # MCP server (HTTP entrypoint)
```

Both images contain the full platform: 11 crates, 10 plugins, 48 actions, examples, GameDays.

```bash
# Run CLI commands
docker run --rm ghcr.io/mwigge/tumult discover
docker run --rm ghcr.io/mwigge/tumult --help

# Start MCP server for agent access
docker run -p 3100:3100 --network tumult-e2e ghcr.io/mwigge/tumult-mcp
```

### Option C: Clone and build

```bash
git clone https://github.com/mwigge/tumult.git && cd tumult
cargo build --release -p tumult-cli -p tumult-mcp
```

### 2. Start infrastructure

```bash
make up-targets
```

This starts 4 chaos targets on the `tumult-e2e` Docker network:

| Service | Port | Credentials |
|---------|------|-------------|
| PostgreSQL 16 | localhost:15432 | tumult / tumult_test |
| Redis 7 | localhost:16379 | — |
| Kafka 3.8 | localhost:19092 | — |
| SSH Server | localhost:12222 | `make ssh-key` for key |

### 3. Run your first chaos experiment

**Redis resilience test** — verify Redis handles a disruption and recovers:

```bash
tumult run examples/redis-chaos.toon
```

Output:
```
Running experiment: Redis resilience — verify recovery after disruption
Status: Completed
Duration: 297ms
Method steps: 3 executed
Journal written to: journal.toon
```

**PostgreSQL failover** — kill idle connections and verify PG recovers:

```bash
tumult run examples/postgres-failover.toon
```

**Pumba network latency** — inject 200ms latency into a container:

```bash
tumult run examples/pumba-latency.toon
```

**SSH remote stress test** — run stress-ng on a remote host via SSH:

```bash
make ssh-key  # extract test SSH key first
tumult run examples/ssh-remote.toon
```

### 4. Explore your data

```bash
# SQL analytics over all experiments
tumult analyze --query "SELECT title, status, duration_ms FROM experiments ORDER BY started_at_ns DESC"

# Export to Parquet for BI tools
tumult export --format parquet journal.toon

# Generate HTML report
tumult report --format html journal.toon

# Compliance evidence (DORA, NIS2, PCI-DSS, ISO-27001, SOC2, ISO-22301, Basel III)
tumult compliance --framework dora .
```

### 5. See what's available

```bash
# List all 10 plugins and 48 actions
tumult discover

# Create your own experiment interactively
tumult init
```

## Run a GameDay (full e2e)

One command — starts infrastructure, runs 4 PostgreSQL resilience experiments via MCP, scores results, maps to DORA compliance:

```bash
./scripts/gameday-demo.sh
```

Output:
```
GameDay: Q2 PostgreSQL Resilience Programme
Status: COMPLIANT
Resilience Score: 1.00
  #1 [PASS] PostgreSQL connection kill under load (2197ms)
  #2 [PASS] PostgreSQL container pause — total unavailability (7402ms)
  #3 [PASS] PostgreSQL CPU stress — resource pressure (9331ms)
  #4 [PASS] PostgreSQL memory stress — resource pressure (9305ms)

Compliance: DORA EU 2022/2554 Art. 11, 24, 25 | NIS2
```

The demo script exercises the full pipeline: Agent → MCP HTTP → experiment runner → plugins → Docker targets → DuckDB analytics → compliance mapping.

## Add observability

Start the full stack with SigNoz dashboards:

```bash
./start.sh infra observe
```

Then run experiments with OpenTelemetry tracing:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317 tumult run examples/redis-chaos.toon
```

Open SigNoz at http://localhost:3301 to see traces, metrics, and dashboards.

| Endpoint | What |
|----------|------|
| localhost:3301 | SigNoz UI (traces, metrics, logs) |
| localhost:14317 | OTLP gRPC (send traces here) |
| localhost:18889 | Prometheus metrics (host + APM) |
| localhost:13133 | Collector health check |

## Bring your own target

To test your own service, create an experiment that probes it:

```toon
title: My service health check
description: Verify my-service handles connection loss

tags[1]: my-service

steady_state_hypothesis:
  title: Service responds 200
  probes[1]:
    - name: health-check
      activity_type: probe
      provider:
        type: process
        path: curl
        arguments[3]: "-s", "-o", "/dev/null -w %{http_code} http://localhost:8080/health"
        timeout_s: 5.0
      tolerance:
        type: regex
        pattern: "200"

method[1]:
  - name: kill-dependency
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "docker stop my-dependency-container"
      timeout_s: 10.0
    pause_after_s: 5.0

rollbacks[1]:
  - name: restart-dependency
    activity_type: action
    provider:
      type: process
      path: sh
      arguments[2]: "-c", "docker start my-dependency-container"
      timeout_s: 10.0
```

```bash
tumult validate my-experiment.toon  # check syntax
tumult run my-experiment.toon       # execute
```

## Stop everything

```bash
make down
```

## Next steps

- [Full documentation](https://mwigge.github.io/tumult/)
- [Plugin reference](docs/plugins/)
- [Experiment format](docs/reference/)
- [Test protocol](docs/testprotocol.md) — 166 platform tests
- [Security assessment](docs/security-assessment.md)
