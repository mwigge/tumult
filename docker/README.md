# Tumult Docker Infrastructure

Two composable stacks: **chaos targets** (what experiments run against) and **observability platform** (SigNoz — traces, metrics, logs in one UI).

## Architecture

```
docker-compose.yml                 docker-compose.observability.yml
(chaos targets)                    (shippable observability platform)
┌────────────┐                     ┌──────────────────────────────────┐
│ PostgreSQL │                     │ SigNoz (UI + backend)    :13301 │
│ Redis      │──── experiments ──>│ SigNoz OTel Collector            │
│ Kafka      │     & probes       │ ClickHouse (storage)             │
│ SSH Server │                     │ ZooKeeper                        │
└────────────┘                     │                                  │
                                   │ Tumult OTel Collector     :14317│
                                   │  ├─ postgresql receiver         │
                                   │  ├─ redis receiver              │
                                   │  ├─ kafkametrics receiver       │
                                   │  ├─ docker_stats receiver       │
                                   │  └─ hostmetrics receiver        │
                                   └──────────────────────────────────┘
```

## Quick Start

```bash
# Full platform (chaos targets + SigNoz observability)
make up

# Open SigNoz — traces, metrics, logs in one UI
open http://localhost:13301

# Run an experiment with OTel export
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317 tumult run experiment.toon

# Stop everything
make down
```

## Compose Modes

| Command | What starts | Use case |
|---------|-------------|----------|
| `make up` | Targets + SigNoz + OTel Collector | Full platform experience |
| `make up-targets` | PostgreSQL, Redis, Kafka, SSH only | Minimal chaos testing |
| `make up-observe` | SigNoz + OTel Collector only | Attach to existing infra |
| `make up-classic` | Targets + Jaeger + Prometheus + Grafana | Legacy/lightweight stack |

## Services

### Chaos Targets (`docker-compose.yml`)

| Service | Port | Purpose |
|---------|------|---------|
| PostgreSQL 16 | `localhost:15432` | Database chaos — kill connections, lock tables |
| Redis 7 | `localhost:16379` | Cache chaos — FLUSHALL, CLIENT PAUSE |
| Kafka 3.8 (KRaft) | `localhost:19092` | Broker chaos — kill broker, partition |
| SSH Server | `localhost:12222` | Remote execution — stress, process chaos |

### Observability Platform (`docker-compose.observability.yml`)

| Service | Port | Purpose |
|---------|------|---------|
| **SigNoz** (standalone) | `localhost:3301` | All-in-one: UI + ClickHouse + OTel Collector + ZooKeeper |
| **Tumult OTel Collector** (contrib) | `localhost:14317` | OTLP gateway → SigNoz + ClickHouse + Prometheus (no build required) |
| Prometheus metrics | `localhost:18889` | Host metrics + APM span metrics |
| Health check | `localhost:13133` | Tumult collector health |

### Classic Profile (optional)

| Service | Port | Purpose |
|---------|------|---------|
| Jaeger | `localhost:16686` | Trace UI |
| Prometheus | `localhost:19090` | Metrics query |
| Grafana | `localhost:13000` | Dashboards (admin/tumult) |

## Infrastructure Metrics

The Tumult OTel Collector automatically scrapes all chaos targets:

| Source | Receiver | Key Metrics |
|--------|----------|-------------|
| PostgreSQL | `postgresql` | connections, rows, locks, WAL size |
| Redis | `redis` | connected_clients, used_memory, ops/sec |
| Kafka | `kafkametrics` | broker count, topic partitions, consumer lag |
| Docker | `docker_stats` | CPU, memory, network I/O per container |
| Host | `hostmetrics` | CPU, memory, disk, filesystem, network |

All metrics flow to SigNoz where you can build dashboards, set alerts, and correlate with experiment traces.

## Environment Variables

```bash
# PostgreSQL
export TUMULT_PG_HOST=localhost
export TUMULT_PG_PORT=15432
export TUMULT_PG_USER=tumult
export TUMULT_PG_PASSWORD=tumult_test
export TUMULT_PG_DATABASE=tumult_test

# Redis
export TUMULT_REDIS_HOST=localhost
export TUMULT_REDIS_PORT=16379

# Kafka
export TUMULT_KAFKA_BOOTSTRAP=localhost:19092

# OTel
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317
```

## SSH Server

- **User:** `tumult` (key-based auth only)
- **Tools:** `stress-ng`, `procps`, `coreutils`
- **Key:** `make ssh-key` → `/tmp/tumult-test-key`

## Shipping as a Platform

The observability stack (`docker-compose.observability.yml`) is designed to be **shippable independently**. Teams can:

1. Run `make up-observe` to start SigNoz + OTel Collector
2. Point Tumult at `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317`
3. Get persistent traces, metrics, and logs from all experiments
4. Build custom dashboards in SigNoz for resilience scoring and compliance

No chaos targets needed — the observability platform works with any Tumult installation.

## License

- **SigNoz core:** MIT License (https://github.com/SigNoz/signoz)
- **SigNoz enterprise (`ee/` directory):** SigNoz Enterprise License (dev/testing permitted)
- **All other images:** Apache 2.0, MIT, or BSD (standard OSS infrastructure)

## Troubleshooting

| Issue | Fix |
|-------|-----|
| Port conflict | Change ports in compose files |
| Kafka slow to start | ~30s for KRaft init: `docker compose logs kafka` |
| SigNoz empty | Wait 30s for first scrape cycle |
| ClickHouse OOM | Increase Docker memory limit to 4GB+ |
| Docker metrics missing | Ensure `/var/run/docker.sock` is mounted |
