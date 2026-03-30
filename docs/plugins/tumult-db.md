# tumult-db — Database Chaos

Script-based plugins for PostgreSQL, MySQL, and Redis chaos engineering.

## Data Capture

All probes return structured JSON output that gets recorded in the experiment journal. When used as hypothesis probes, the data is captured at both the **before** (baseline) and **after** (post-fault) checkpoints, giving you a pre/post comparison.

For continuous during-phase sampling, use probes in the method section with `background: true` (Phase 6 feature).

## PostgreSQL (tumult-db-postgres)

### Prerequisites
- `psql` client installed
- Network access to the PostgreSQL instance

### Actions

| Action | Description | Key Variables |
|--------|-------------|---------------|
| `kill-connections` | Terminate all connections to a database | `TUMULT_PG_DATABASE` (required) |
| `lock-table` | Exclusive lock for a duration | `TUMULT_PG_DATABASE`, `TUMULT_PG_TABLE`, `TUMULT_DURATION` |
| `inject-latency` | pg_sleep to simulate slow queries | `TUMULT_PG_DATABASE`, `TUMULT_LATENCY_MS` |
| `exhaust-connections` | Open N idle connections | `TUMULT_PG_DATABASE`, `TUMULT_CONNECTION_COUNT`, `TUMULT_DURATION` |

### Probes

| Probe | Description | Output |
|-------|-------------|--------|
| `connection-count` | Active connection count | Integer |
| `replication-lag` | Replica lag in seconds | Float (0 if primary) |
| `pool-utilization` | Connection pool stats | JSON: `{current_connections, max_connections, utilization_pct}` |

### Common Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TUMULT_PG_HOST` | `localhost` | PostgreSQL host |
| `TUMULT_PG_PORT` | `5432` | PostgreSQL port |
| `TUMULT_PG_USER` | `postgres` | PostgreSQL user |
| `TUMULT_PG_PASSWORD` | (empty) | Password (uses PGPASSWORD) |

## MySQL (tumult-db-mysql)

### Prerequisites
- `mysql` client installed

### Actions

| Action | Description | Key Variables |
|--------|-------------|---------------|
| `kill-connections` | Kill connections via KILL command | `TUMULT_MYSQL_DATABASE` (required) |
| `lock-table` | LOCK TABLES WRITE for a duration | `TUMULT_MYSQL_DATABASE`, `TUMULT_MYSQL_TABLE`, `TUMULT_DURATION` |

### Probes

| Probe | Description | Output |
|-------|-------------|--------|
| `connection-count` | Active connection count | Integer |

## Redis (tumult-db-redis)

### Prerequisites
- `redis-cli` installed

### Actions

| Action | Description | Key Variables |
|--------|-------------|---------------|
| `flush-all` | FLUSHALL — delete all data | (destructive!) |
| `block-clients` | CLIENT PAUSE for N ms | `TUMULT_DURATION` (ms) |
| `simulate-failover` | DEBUG SLEEP — hang Redis | `TUMULT_DURATION` (seconds) |

### Probes

| Probe | Description | Output |
|-------|-------------|--------|
| `redis-ping` | PING/PONG liveness check | `"PONG"` or error |
| `redis-info` | Connection + memory stats | JSON: `{connected_clients, used_memory_bytes, ops_per_sec, ...}` |

## Example: PostgreSQL Connection Pool Exhaustion

```toon
title: Application survives connection pool exhaustion
description: Exhaust PostgreSQL connections and verify app recovers

steady_state_hypothesis:
  title: Database pool has capacity
  probes[1]:
    - name: pool-check
      activity_type: probe
      provider:
        type: process
        path: plugins/tumult-db-postgres/probes/pool-utilization.sh
        env:
          TUMULT_PG_DATABASE: myapp
      tolerance:
        type: range
        from: 0.0
        to: 80.0

method[1]:
  - name: exhaust-pool
    activity_type: action
    provider:
      type: process
      path: plugins/tumult-db-postgres/actions/exhaust-connections.sh
      env:
        TUMULT_PG_DATABASE: myapp
        TUMULT_CONNECTION_COUNT: 80
        TUMULT_DURATION: 30
```
