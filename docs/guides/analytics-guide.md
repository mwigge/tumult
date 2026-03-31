---
title: Analytics Guide
parent: Guides
nav_order: 5
---

# Analytics Guide

Tumult embeds DuckDB and Apache Arrow for SQL analytics over experiment journals.

## Architecture

```
TOON Journal (.toon) → Arrow RecordBatch (columnar) → DuckDB (SQL) → Parquet (export)
```

## Tables

### experiments

| Column | Type | Description |
|--------|------|-------------|
| experiment_id | VARCHAR | UUID per run |
| title | VARCHAR | Experiment title |
| status | VARCHAR | Completed, Deviated, Aborted, Failed |
| started_at_ns | BIGINT | Start time (epoch nanoseconds) |
| ended_at_ns | BIGINT | End time |
| duration_ms | UBIGINT | Total duration |
| method_step_count | BIGINT | Method steps executed |
| rollback_count | BIGINT | Rollback steps executed |
| hypothesis_before_met | BOOLEAN | Steady state before? |
| hypothesis_after_met | BOOLEAN | Steady state after? |
| estimate_accuracy | DOUBLE | Estimate vs actual (0-1) |
| resilience_score | DOUBLE | Overall resilience (0-1) |

### activity_results

| Column | Type | Description |
|--------|------|-------------|
| experiment_id | VARCHAR | Links to experiments |
| name | VARCHAR | Activity name |
| activity_type | VARCHAR | Action or Probe |
| status | VARCHAR | Succeeded, Failed, Timeout |
| started_at_ns | BIGINT | Start time |
| duration_ms | UBIGINT | Execution duration |
| output | VARCHAR | Activity output |
| error | VARCHAR | Error message |
| phase | VARCHAR | hypothesis_before, method, hypothesis_after, rollback |

## Example Queries

```sql
-- Summary by status
SELECT status, count(*) as runs, avg(duration_ms) as avg_ms
FROM experiments GROUP BY status;

-- Slowest activities
SELECT name, phase, avg(duration_ms) as avg_ms
FROM activity_results GROUP BY name, phase ORDER BY avg_ms DESC LIMIT 10;

-- Failure rate
SELECT count(*) FILTER (WHERE status != 'Completed')::float / count(*) * 100 as failure_pct
FROM experiments;
```

## Persistent Store

Every `tumult run` automatically ingests the journal into a persistent DuckDB store at `~/.tumult/analytics.duckdb`. This enables cross-run analytics without manually specifying journal paths.

```bash
# Query the persistent store (no path needed)
tumult analyze --query "SELECT status, count(*) FROM experiments GROUP BY status"

# View store statistics
tumult store stats

# Show store path
tumult store path
```

### Backup and Restore

```bash
# Export the entire store to Parquet files
tumult store backup --output my-backup/

# Restore from backup into the persistent store
tumult import my-backup/
```

### Retention

```bash
# Purge experiments older than 90 days
tumult store purge --older-than-days 90
```

### Disabling Auto-Ingest

```bash
# Run without ingesting into persistent store
tumult run experiment.toon --no-ingest
```

## CLI Usage

```bash
# Default summary (from persistent store)
tumult analyze

# Load from journal files
tumult analyze journals/

# Custom SQL
tumult analyze journals/ --query "SELECT * FROM experiments WHERE status = 'Deviated'"

# Single journal
tumult analyze journal.toon

# Export to Parquet
tumult export journal.toon --format parquet

# Export to CSV
tumult export journal.toon --format csv

# Trend analysis
tumult trend journals/ --metric resilience_score --last 30d
```
