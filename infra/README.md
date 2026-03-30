# Test Infrastructure

Docker Compose environment for Tumult e2e testing.

## Services

| Service | Port | Purpose |
|---------|------|---------|
| PostgreSQL 16 | 15432 | Database chaos target + probe testing |
| Redis 7 | 16379 | Cache chaos target + probe testing |
| Kafka 3.8 (KRaft) | 19092 | Broker chaos + consumer lag probes |
| SSH server | 12222 | Remote execution testing |
| OTel Collector | 14317 | OTLP receiver for trace verification |
| Jaeger | 16686 | Trace UI for OTel verification |

## Usage

```bash
cd infra/
docker compose up -d        # Start all services
docker compose ps            # Check health
docker compose down -v       # Stop and clean up
```

## E2E Test Environment Variables

```bash
export TUMULT_PG_HOST=localhost
export TUMULT_PG_PORT=15432
export TUMULT_PG_USER=tumult
export TUMULT_PG_PASSWORD=tumult_test
export TUMULT_PG_DATABASE=tumult_test

export TUMULT_REDIS_HOST=localhost
export TUMULT_REDIS_PORT=16379

export TUMULT_KAFKA_BOOTSTRAP=localhost:19092

export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317
```

## Status

This infrastructure is planned but not yet wired to automated e2e tests.
See `openspec/changes/tumult-platform-architecture/tasks.md` Phase 7 for the task list.
