# Analytics Guide

Tumult embeds DuckDB and Apache Arrow for SQL analytics over experiment journals. No external database needed — everything runs inside the `tumult` binary.

## Architecture

```
TOON Journal (.toon)
    │
    ▼ parse + convert
Arrow RecordBatch (in-memory, columnar)
    │
    ├──▶ DuckDB (embedded SQL engine)
    │      • tumult analyze — ad-hoc queries
    │      • tumult trend — cross-run analysis (Phase 4)
    │      • tumult compliance — regulatory reports (Phase 5)
    │
    ├──▶ Parquet (compressed columnar file)
    │      • tumult export --format parquet
    │      • Readable by Spark, Polars, pandas, DuckDB CLI
    │
    └──▶ CSV (flat file)
           • tumult export --format csv
```

## Tables

### experiments

One row per experiment run.

| Column | Type | Description |
|--------|------|-------------|
| experiment_id | VARCHAR | UUID per run |
| title | VARCHAR | Experiment title |
| status | VARCHAR | Completed, Deviated, Aborted, Failed |
| started_at_ns | BIGINT | Start time (epoch nanoseconds) |
| ended_at_ns | BIGINT | End time (epoch nanoseconds) |
| duration_ms | UBIGINT | Total duration |
| method_step_count | BIGINT | Number of method steps |
| rollback_count | BIGINT | Number of rollback steps |
| hypothesis_before_met | BOOLEAN | Did steady state hold before? |
| hypothesis_after_met | BOOLEAN | Did steady state hold after? |
| estimate_accuracy | DOUBLE | Estimate vs actual (0-1) |
| resilience_score | DOUBLE | Overall resilience score (0-1) |

### activity_results

One row per activity execution (probes, actions, rollbacks).

| Column | Type | Description |
|--------|------|-------------|
| experiment_id | VARCHAR | Links to experiments table |
| name | VARCHAR | Activity name |
| activity_type | VARCHAR | Action or Probe |
| status | VARCHAR | Succeeded, Failed, Timeout |
| started_at_ns | BIGINT | Start time |
| duration_ms | UBIGINT | Execution duration |
| output | VARCHAR | Activity output (probe data) |
| error | VARCHAR | Error message if failed |
| phase | VARCHAR | hypothesis_before, method, hypothesis_after, rollback |

## Example Queries

### Summary by status

```sql
SELECT status, count(*) as runs, avg(duration_ms) as avg_ms
FROM experiments
GROUP BY status
ORDER BY runs DESC;
```

### Slowest activities

```sql
SELECT name, phase, avg(duration_ms) as avg_ms, max(duration_ms) as max_ms
FROM activity_results
GROUP BY name, phase
ORDER BY avg_ms DESC
LIMIT 10;
```

### Failure rate over time

```sql
SELECT
    date_trunc('day', epoch_ns(started_at_ns)) as day,
    count(*) as total,
    count(*) FILTER (WHERE status != 'Completed') as failures,
    round(count(*) FILTER (WHERE status != 'Completed')::float / count(*) * 100, 1) as failure_pct
FROM experiments
GROUP BY day
ORDER BY day;
```

### Resilience trend

```sql
SELECT experiment_id, title, resilience_score
FROM experiments
WHERE resilience_score IS NOT NULL
ORDER BY started_at_ns;
```

## CLI Usage

```bash
# Default summary (status breakdown + phase stats)
tumult analyze journals/

# Custom SQL query
tumult analyze journals/ --query "SELECT * FROM experiments WHERE status = 'Deviated'"

# Analyze a single journal
tumult analyze journal.toon

# Export to Parquet
tumult export journal.toon --format parquet

# Export to CSV
tumult export journal.toon --format csv
```

## Future: Persistent Store (Phase 4)

In Phase 4, `tumult analyze` will use a persistent DuckDB file at `~/.tumult/analytics.duckdb`. Journals will be auto-ingested after each `tumult run`, enabling cross-run trend analysis without re-scanning journal files.
