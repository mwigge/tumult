# Tumult Docker Infrastructure

Four composable bundles for a complete chaos engineering lab.

## Pre-built Images

Published to [GitHub Container Registry](https://github.com/mwigge?tab=packages) on every release:

```bash
docker pull ghcr.io/mwigge/tumult:latest        # CLI + MCP server
docker pull ghcr.io/mwigge/tumult-mcp:latest     # MCP server (HTTP entrypoint)
```

Both images contain the full platform — all 11 crates, 10 plugins, 45 actions,
example experiments, and GameDay definitions.

## Architecture

```
┌────────────────────────────────────────────────────────────────────────────┐
│  ./start.sh all                                                            │
├──────────────────┬──────────────────┬─────────────────┬────────────────────┤
│  infra           │  observe         │  tumult         │  aqe               │
│  (chaos targets) │  (observability) │  (MCP server)   │  (agent fleet)     │
│                  │                  │                 │                    │
│  PostgreSQL 16   │  SigNoz UI :3301 │  tumult-mcp     │  Agentic QE Fleet  │
│  :15432          │                  │  :3100 (HTTP)   │                    │
│                  │  OTel Collector  │                 │  → tumult-mcp:3100 │
│  Redis 7         │  :14317 (OTLP)   │  14 MCP tools   │                    │
│  :16379          │  :18889 (prom)   │  DuckDB store   │                    │
│                  │                  │  10 plugins     │                    │
│  Kafka 3.8       │  ClickHouse      │  45 actions     │                    │
│  :19092          │  (in SigNoz)     │                 │                    │
│                  │                  │                 │                    │
│  SSH Server      │                  │                 │                    │
│  :12222          │                  │                 │                    │
└──────────────────┴──────────────────┴─────────────────┴────────────────────┘
                              │                  │
                              ▼                  ▼
                  ┌──────────────────────────────────────┐
                  │  tumult-e2e Docker network            │
                  └──────────────────────────────────────┘
```

## Quick Start

```bash
# Full e2e environment (chaos targets + observability)
./start.sh

# Run a chaos experiment with OTel traces
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317
tumult run examples/postgres-failover.toon

# View traces in SigNoz
open http://localhost:3301

# Analyze results
tumult analyze --all

# Start MCP server for agent access
./start.sh tumult
# → http://localhost:3100/mcp

# Stop everything
./start.sh down
```

## Bundles

| Bundle | Compose File | Command | What starts |
|--------|-------------|---------|-------------|
| **infra** | `docker-compose.yml` | `./start.sh infra` | PG, Redis, Kafka, SSH |
| **observe** | `docker-compose.observability.yml` | `./start.sh observe` | SigNoz + OTel Collector |
| **tumult** | `docker-compose.tumult.yml` | `./start.sh tumult` | MCP server (HTTP/SSE) |
| **aqe** | `docker-compose.aqe.yml` | `./start.sh aqe` | Agentic QE Fleet |

Combine freely: `./start.sh infra observe tumult`

## Port Map

All ports use the `1xxxx` range to avoid conflicts with local services.

| Bundle | Service | Port | Purpose |
|--------|---------|------|---------|
| infra | PostgreSQL 16 | 15432 | Database chaos target |
| infra | Redis 7 | 16379 | Cache chaos target |
| infra | Kafka 3.8 (KRaft) | 19092 | Message broker chaos target |
| infra | SSH Server | 12222 | Remote execution target |
| observe | SigNoz UI | 3301 | Traces, metrics, logs dashboard |
| observe | OTel Collector (OTLP) | 14317 | OTLP gRPC ingest |
| observe | OTel Collector (Prometheus) | 18889 | Host + APM span metrics |
| observe | OTel Collector (health) | 13133 | Health check endpoint |
| tumult | MCP Server (HTTP/SSE) | 3100 | MCP tools for agents |
| classic | Jaeger | 16686 | Trace visualization (opt) |
| classic | Grafana | 13000 | Dashboards (opt) |

## OTel Data Flow

```
tumult run experiment.toon
    │
    │ 7 canonical spans:
    │   resilience.experiment
    │   resilience.hypothesis.before / .after
    │   resilience.action / .probe / .rollback
    │   resilience.analytics.ingest
    │
    ▼ OTLP gRPC :14317
┌───────────────────────┐
│  OTel Collector       │  Contrib image (no build needed):
│  (tumult-collector)   │  - OTLP + Arrow receivers
│                       │  - Span-to-metrics (APM)
│                       │  - Host metrics
│                       │  - Prometheus exporter (:18889)
└───────────┬───────────┘
            │ OTLP gRPC :4317
            ▼
┌───────────────────────┐
│  SigNoz Standalone    │  All-in-one:
│  (all-in-one)         │  - ClickHouse storage
│                       │  - Trace explorer
│                       │  - Metrics dashboard
│  UI: :3301            │  - Alerting
└───────────────────────┘
```

## Files

| File | Purpose |
|------|---------|
| `docker-compose.yml` | Chaos targets (PG, Redis, Kafka, SSH) |
| `docker-compose.observability.yml` | SigNoz + OTel Collector + classic profile |
| `docker-compose.tumult.yml` | Tumult MCP server |
| `docker-compose.aqe.yml` | Agentic QE Fleet |
| `Dockerfile.tumult` | CLI + MCP image (both binaries) |
| `Dockerfile.tumult-mcp` | MCP server entrypoint variant |
| `Dockerfile.sshd` | SSH test target |
| `tumult-collector/config.yaml` | OTel Collector pipeline config |
| `init-postgres.sql` | PostgreSQL test schema |
| `prometheus.yml` | Prometheus scrape config (classic) |
| `grafana/` | Grafana provisioning (classic) |
| `signoz/` | SigNoz dashboard definitions |

## Environment Variables

```bash
# PostgreSQL
TUMULT_PG_HOST=localhost TUMULT_PG_PORT=15432
TUMULT_PG_USER=tumult TUMULT_PG_PASSWORD=tumult_test

# Redis
TUMULT_REDIS_HOST=localhost TUMULT_REDIS_PORT=16379

# Kafka
TUMULT_KAFKA_BOOTSTRAP=localhost:19092

# OTel
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317

# MCP auth (optional)
TUMULT_MCP_TOKEN=my-secret
```

## SSH Server

- **User:** `tumult` (key-based auth only)
- **Tools:** `stress-ng`, `procps`, `coreutils`
- **Key:** `make ssh-key` → `/tmp/tumult-test-key`

## Troubleshooting

| Issue | Fix |
|-------|-----|
| Port conflict | Change ports in compose files |
| Docker build slow | Use pre-built GHCR images: `docker compose pull` |
| Kafka slow to start | ~30s for KRaft init: `docker compose logs kafka` |
| SigNoz empty | Wait 30s for first scrape cycle |
| ClickHouse OOM | Increase Docker memory limit to 4GB+ |
