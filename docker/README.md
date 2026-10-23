# Tumult Docker Infrastructure

For guided browser lessons with an optional AI tutor, start with the
[Chaos Lab](../chaos-lab/README.md). It needs only Docker to run the experiments.

Four composable bundles for a complete chaos engineering lab.

## Pre-built Images

Published to [GitHub Container Registry](https://github.com/mwigge?tab=packages) on every release:

```bash
docker pull ghcr.io/mwigge/tumult:latest        # CLI + MCP server
docker pull ghcr.io/mwigge/tumult-mcp:latest     # MCP server (HTTP entrypoint)
```

Both images contain `tumult`, `tumult-mcp` and `tumult-net-proxyd`, plus 11
script plugins, example experiments and GameDay definitions. The repository has
32 workspace crates; these images do not include `tumultd` or its web UI.
With the bundled manifests, `tumult discover` lists 16 plugins (11 script and
5 native) and 91 actions (56 script actions plus 35 native functions, including
probes). Provider tools, permissions and the host OS determine what can execute.
The daemon and embedded web UI ship separately as `ghcr.io/mwigge/tumultd:latest`.

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
│  Redis 7         │  :14317 (OTLP)   │  40 MCP tools   │                    │
│  :16379          │  :18889 (prom)   │  DuckDB store   │                    │
│  MySQL 8 :13306  │                  │  11 script      │                    │
│  Kafka 3.8       │  ClickHouse      │  plugins        │                    │
│  :19092          │  (in SigNoz)     │                 │                    │
│                  │                  │  91 listed      │                    │
│  SSH Server      │                  │  actions        │                    │
│  :12222          │                  │                 │                    │
└──────────────────┴──────────────────┴─────────────────┴────────────────────┘
                              │                  │
                              ▼                  ▼
                  ┌──────────────────────────────────────┐
                  │  tumult-e2e Docker network            │
                  └──────────────────────────────────────┘
```

## Quick Start

Run these commands from the repository root; `start.sh` is not in `docker/`.

```bash
# Required only for the tumultd demo stack (docker-compose.kronika.yml):
# copy the template and fill in the three KRONIKA_* values.
cp .env.example .env

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
export OTEL_EXPORTER_OTLP_ENDPOINT=http://tumult-collector:4317
./start.sh tumult
# → http://localhost:3100/mcp

# If aqe was started, stop it separately before removing the network:
# docker compose -f docker/docker-compose.aqe.yml down
# Stop infra, observe and tumult (does not stop aqe)
./start.sh down
```

The MCP compose file's fallback hostname is `otel-collector`; the observe bundle
names its collector `tumult-collector`, so set the endpoint as above. The MCP
and aqe bundles need the `tumult-e2e` network; start infra first. The aqe compose
file hardcodes `http://otel-collector:4317`, so a host `export` does not reach it;
add an override file and pass it after the base file:

```yaml
# docker/docker-compose.aqe.override.yml
services:
  agentic-qe:
    environment:
      OTEL_EXPORTER_OTLP_ENDPOINT: http://tumult-collector:4317
```

```bash
docker compose -f docker/docker-compose.aqe.yml -f docker/docker-compose.aqe.override.yml up -d
```
The MCP image is a thin layer over a local `tumult` base when built: pull the published
image with `docker compose -f docker/docker-compose.tumult.yml pull`, or build
the base with `docker build -f docker/Dockerfile.tumult -t tumult .` first.

## Bundles

| Bundle | Compose File | Command | What starts |
|--------|-------------|---------|-------------|
| **infra** | `docker-compose.yml` | `./start.sh infra` | PG, Redis, MySQL, Kafka, SSH |
| **observe** | `docker-compose.observability.yml` | `./start.sh observe` | SigNoz + OTel Collector |
| **tumult** | `docker-compose.tumult.yml` | `./start.sh tumult` | MCP server (Streamable HTTP) |
| **aqe** | `docker-compose.aqe.yml` | `./start.sh aqe` | Agentic QE Fleet (requires sibling `../agentic-qe` clone) |

Combine freely from the repository root: `./start.sh infra observe tumult`.
`./start.sh all` includes aqe and fails if its sibling clone is missing.
The daemon demo is separate: `docker compose -f docker/docker-compose.kronika.yml up -d --build`.

## Port Map

Target and collector ports mostly use the `1xxxx` range. SigNoz and MCP use
the exceptions below. The observe and daemon demo stacks both publish
`14317`/`14318`; do not start both with their default port mappings.

| Bundle | Service | Port | Purpose |
|--------|---------|------|---------|
| infra | PostgreSQL 16 | 15432 | Database chaos target |
| infra | Redis 7 | 16379 | Cache chaos target |
| infra | MySQL 8 | 13306 | Database chaos target |
| infra | Kafka 3.8 (KRaft) | 19092 | Message broker chaos target |
| infra | SSH Server | 12222 | Remote execution target |
| observe | SigNoz UI | 3301 | Traces, metrics, logs dashboard |
| observe | OTel Collector (OTLP) | 14317 | OTLP gRPC ingest |
| observe | OTel Collector (OTLP HTTP) | 14318 | OTLP HTTP ingest |
| observe | OTel Collector (Arrow) | 14319 | OTel Arrow gRPC ingest |
| observe | OTel Collector (Prometheus) | 18889 | Host + APM span metrics |
| observe | OTel Collector (health) | 13133 | Health check endpoint |
| observe | OTel Collector (self-telemetry) | 18888 | Collector metrics |
| observe | SigNoz OTLP | 4317 / 4318 | Direct gRPC / HTTP ingest |
| tumult | MCP Server (Streamable HTTP) | 3100 | MCP tools for agents |
| daemon demo | tumultd | 14317 / 14318 | OTLP gRPC / HTTP, web UI and JSON API |
| classic | Jaeger | 16686 | Trace visualization (opt) |
| classic | Prometheus | 19090 | Scrape dashboard (opt) |
| classic | Grafana | 13000 | Dashboards (opt) |

## OTel Data Flow

```
tumult run experiment.toon
    │
    │ Core execution and ingest span names (not a fixed per-run count):
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
| `docker-compose.yml` | Chaos targets (PG, Redis, MySQL, Kafka, SSH) |
| `docker-compose.observability.yml` | SigNoz + OTel Collector + classic profile |
| `docker-compose.tumult.yml` | Tumult MCP server |
| `docker-compose.aqe.yml` | Agentic QE Fleet |
| `docker-compose.kronika.yml` | tumultd demo stack (daemon + web UI + seed + report) |
| `docker-compose.kronika-collector.yml` | tumultd dev pair with OTel Collector (`otel-collector-kronika.yaml`) |
| `docker-compose.grafana-full.yml` | Grafana stack reference: Tempo + Mimir + Loki + Grafana (see `docs/guides/grafana-stack.md`) |
| `Dockerfile.tumult` | CLI + MCP image (both binaries) |
| `Dockerfile.tumultd` | tumultd image (web UI build + daemon + CLI) |
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
export TUMULT_PG_HOST=localhost TUMULT_PG_PORT=15432
export TUMULT_PG_USER=tumult TUMULT_PG_PASSWORD=tumult_test
export TUMULT_PG_DATABASE=tumult_test

# Redis
export TUMULT_REDIS_HOST=localhost TUMULT_REDIS_PORT=16379

# Kafka
export TUMULT_KAFKA_BOOTSTRAP=localhost:19092

# OTel
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317

# MCP auth (compose defaults to a local development token; replace it)
export TUMULT_MCP_TOKEN=replace-with-a-secret
```

## SSH Server

- **User:** `tumult` (key-based auth only)
- **Tools:** `stress-ng`, `procps`, `coreutils`
- **Key:** `make ssh-key` from the repository root extracts the container's
  generated test key to `/tmp/tumult-test-key`.

## Troubleshooting

| Issue | Fix |
|-------|-----|
| Port conflict | Change ports in compose files |
| Docker build slow | Use pre-built GHCR images: `docker compose pull` |
| Kafka slow to start | ~30s for KRaft init: `docker compose logs kafka` |
| SigNoz empty | Wait 30s for first scrape cycle |
| ClickHouse OOM | Increase Docker memory limit to 4GB+ |
