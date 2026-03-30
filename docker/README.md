# Tumult Docker Test Infrastructure

Docker Compose environment providing all services needed for end-to-end testing of chaos experiments.

## Services

| Service | Image | Port | Purpose |
|---------|-------|------|---------|
| **PostgreSQL 16** | `postgres:16-alpine` | `localhost:15432` | Database chaos target — kill connections, lock tables, pool exhaustion |
| **Redis 7** | `redis:7-alpine` | `localhost:16379` | Cache chaos target — FLUSHALL, CLIENT PAUSE, DEBUG SLEEP |
| **Kafka 3.8** | `apache/kafka:3.8.0` | `localhost:19092` | Broker chaos — kill broker, partition, consumer lag probes |
| **SSH Server** | `tumult-sshd` (custom) | `localhost:12222` | Remote execution — stress tests, process chaos via SSH |
| **OTel Collector** | `otel/opentelemetry-collector-contrib` | `localhost:14317` | OTLP receiver — trace verification |
| **Jaeger** | `jaegertracing/all-in-one` | `localhost:16686` | Trace UI — verify experiment spans |

All ports use the `1xxxx` range to avoid conflicts with locally running services.

## Quick Start

```bash
# Start everything
make infra-up

# Check health
make infra-status

# Extract SSH test key (for SSH remote execution tests)
make ssh-key

# Run e2e tests
make e2e

# View traces
open http://localhost:16686

# Stop everything
make infra-down
```

## Manual Usage

```bash
cd docker/
docker compose up -d
docker compose ps
docker compose down -v
```

## Environment Variables for Tests

Set these when running experiments against the Docker infrastructure:

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

# OTel (traces go to Jaeger via Collector)
export OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:14317
```

## SSH Server

The SSH container includes:
- **User:** `tumult` (key-based auth only, no password)
- **Installed tools:** `stress-ng`, `procps`, `coreutils`
- **Test key:** extracted via `make ssh-key` → `/tmp/tumult-test-key`

```bash
# Test SSH connection
ssh -p 12222 -i /tmp/tumult-test-key -o StrictHostKeyChecking=no tumult@localhost uname -a

# Run stress test via SSH
ssh -p 12222 -i /tmp/tumult-test-key tumult@localhost "stress-ng --cpu 2 --timeout 10s --metrics-brief"
```

## PostgreSQL Test Data

The `init-postgres.sql` creates:
- `app_sessions` table with 5 sample rows
- `connection_stats` view for monitoring
- All permissions granted to the `tumult` user

```bash
# Connect manually
psql -h localhost -p 15432 -U tumult -d tumult_test

# Test the connection count probe
TUMULT_PG_HOST=localhost TUMULT_PG_PORT=15432 TUMULT_PG_USER=tumult \
  TUMULT_PG_PASSWORD=tumult_test \
  sh plugins/tumult-db-postgres/probes/connection-count.sh
```

## Kafka

KRaft mode (no ZooKeeper). Single broker for testing.

```bash
# Create a test topic
docker exec docker-kafka-1 /opt/kafka/bin/kafka-topics.sh \
  --bootstrap-server localhost:9092 --create --topic tumult-test --partitions 3

# List topics
docker exec docker-kafka-1 /opt/kafka/bin/kafka-topics.sh \
  --bootstrap-server localhost:9092 --list
```

## OTel Trace Verification

After running an experiment with `OTEL_EXPORTER_OTLP_ENDPOINT` set:

1. Open Jaeger UI: http://localhost:16686
2. Select service: `tumult`
3. Click "Find Traces"
4. Each experiment produces a trace with `tumult.action` / `tumult.probe` spans

## Cleanup

```bash
# Stop and remove everything (including volumes)
make infra-down

# Or manually
cd docker/ && docker compose down -v
```

## Troubleshooting

| Issue | Fix |
|-------|-----|
| Port conflict | Change ports in `docker-compose.yml` (e.g., `25432:5432`) |
| Kafka slow to start | It needs ~30s for KRaft init. Check `docker compose logs kafka` |
| SSH key permission denied | Run `chmod 600 /tmp/tumult-test-key` |
| OTel traces not appearing | Check collector logs: `docker compose logs otel-collector` |
