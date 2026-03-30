# <img src="../images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> Data-Driven Chaos: SQL Analytics Over Experiment Journals

![Tumult Banner](../images/tumult-banner.png)

*Part 6 of the Tumult series. [← Part 5: Writing Your First Experiment](./05-experiment-format.md)*

---

Most chaos engineering tools answer one question: did this experiment pass or fail? That is necessary, but it is not sufficient. The questions that matter for a production engineering team are different:

- Is our payment service getting more or less resilient over time?
- Which systems are consistently exceeding their recovery time objectives?
- Where are our predictions furthest from reality — and what does that tell us about our mental models?
- Which experiments have never run against production?

Answering these questions requires treating chaos experiment data as data — not as logs to scroll through, but as structured records to query, aggregate, and visualize. Tumult's analytics pipeline is built on this premise.

---

## The Pipeline

Every experiment run produces a structured journal in TOON format. The analytics pipeline transforms those journals into queryable data:

```
Experiment → TOON Journal → Apache Arrow (columnar) → DuckDB (embedded SQL) → Parquet (export)
```

The key properties of this pipeline:

**Arrow**: experiments are parsed into Apache Arrow `RecordBatch` format — columnar, memory-mapped, and interoperable with the entire Arrow ecosystem (Polars, pandas, Spark, BigQuery).

**DuckDB**: an embedded analytical database. No server, no setup, no network. DuckDB runs in-process alongside Tumult, meaning you can run SQL queries against thousands of journals without any infrastructure.

**Parquet**: the export format. Parquet is the standard for portable, compressed columnar data. A Parquet file from Tumult can be opened in Jupyter, loaded into Spark, queried in BigQuery, or archived to S3 — by any tool in the data ecosystem.

---

## The Data Model

Tumult analytics exposes two primary tables.

### `experiments`

One row per experiment run.

| Column | Type | Description |
|--------|------|-------------|
| `experiment_id` | VARCHAR | UUID for this run |
| `title` | VARCHAR | Experiment title |
| `status` | VARCHAR | `Completed`, `Deviated`, `Aborted`, `Failed` |
| `started_at_ns` | BIGINT | Start time (epoch nanoseconds) |
| `ended_at_ns` | BIGINT | End time |
| `duration_ms` | UBIGINT | Total experiment duration |
| `method_step_count` | BIGINT | Method steps executed |
| `rollback_count` | BIGINT | Rollback steps executed |
| `hypothesis_before_met` | BOOLEAN | Was steady state met before fault? |
| `hypothesis_after_met` | BOOLEAN | Was steady state met after fault? |
| `estimate_accuracy` | DOUBLE | Estimate vs actual (0.0–1.0) |
| `resilience_score` | DOUBLE | Overall resilience score (0.0–1.0) |

### `activity_results`

One row per action or probe execution within an experiment.

| Column | Type | Description |
|--------|------|-------------|
| `experiment_id` | VARCHAR | Links to `experiments` |
| `name` | VARCHAR | Activity name |
| `activity_type` | VARCHAR | `Action` or `Probe` |
| `status` | VARCHAR | `Succeeded`, `Failed`, `Timeout` |
| `started_at_ns` | BIGINT | Start time |
| `duration_ms` | UBIGINT | Execution duration |
| `output` | VARCHAR | Activity output |
| `error` | VARCHAR | Error message (if failed) |
| `phase` | VARCHAR | `hypothesis_before`, `method`, `hypothesis_after`, `rollback` |

---

## Running Analytics

### Built-in summary

```bash
# Default summary across all journals in a directory
tumult analyze journals/
```

Output:
```
Experiments: 47
  Completed:  38 (81%)
  Deviated:    7 (15%)
  Aborted:     2 (4%)

Average duration: 3m 24s
Average resilience score: 0.73

Top deviating experiments:
  postgresql-failover          7 deviations / 12 runs
  redis-cache-flush            3 deviations / 8 runs
  kafka-broker-kill            2 deviations / 5 runs
```

### Custom SQL queries

```bash
# Run any SQL query against the journals
tumult analyze journals/ --query "
  SELECT status, count(*) as runs, avg(duration_ms) as avg_ms
  FROM experiments
  GROUP BY status"

# Query a single journal
tumult analyze journal.toon --query "
  SELECT name, phase, duration_ms
  FROM activity_results
  ORDER BY duration_ms DESC"
```

---

## Useful Queries

### Resilience trend over time

```sql
SELECT
    DATE_TRUNC('week', TIMESTAMP 'epoch' + started_at_ns * INTERVAL '1 nanosecond') AS week,
    COUNT(*) AS runs,
    AVG(CASE WHEN status = 'Completed' THEN 1.0 ELSE 0.0 END) AS success_rate,
    AVG(resilience_score) AS avg_score
FROM experiments
GROUP BY week
ORDER BY week;
```

Watching `success_rate` over time tells you whether the engineering organization is making services more resilient or less. A declining trend warrants attention before it becomes an incident.

### Slowest-recovering systems

```sql
SELECT
    title,
    COUNT(*) AS runs,
    AVG(duration_ms) / 1000.0 AS avg_duration_s,
    MAX(duration_ms) / 1000.0 AS worst_s,
    MIN(duration_ms) / 1000.0 AS best_s
FROM experiments
WHERE status IN ('Completed', 'Deviated')
GROUP BY title
ORDER BY avg_duration_s DESC
LIMIT 10;
```

### Activities that consistently fail

```sql
SELECT
    ar.name,
    ar.phase,
    ar.activity_type,
    COUNT(*) AS total_runs,
    SUM(CASE WHEN ar.status = 'Succeeded' THEN 0 ELSE 1 END) AS failures,
    ROUND(
        SUM(CASE WHEN ar.status = 'Succeeded' THEN 0 ELSE 1 END) * 100.0 / COUNT(*), 1
    ) AS failure_pct
FROM activity_results ar
GROUP BY ar.name, ar.phase, ar.activity_type
HAVING failures > 0
ORDER BY failure_pct DESC;
```

If a probe is failing 30% of the time in the `hypothesis_before` phase, it means the system is regularly not healthy before chaos is even injected — a signal worth investigating independently of the chaos experiments.

### Estimate accuracy analysis

```sql
SELECT
    title,
    COUNT(*) AS runs,
    AVG(estimate_accuracy) AS avg_accuracy,
    SUM(CASE WHEN estimate_accuracy = 1.0 THEN 1 ELSE 0 END) AS exact_hits,
    SUM(CASE WHEN estimate_accuracy = 0.0 THEN 1 ELSE 0 END) AS complete_misses
FROM experiments
WHERE estimate_accuracy IS NOT NULL
GROUP BY title
ORDER BY avg_accuracy ASC;
```

Experiments with consistently zero estimate accuracy are the most valuable learning opportunities. They represent scenarios where the team's mental model of the system is meaningfully wrong.

### Hypothesis deviation rate by phase

```sql
SELECT
    CASE
        WHEN NOT hypothesis_before_met THEN 'Failed before fault (Aborted)'
        WHEN NOT hypothesis_after_met THEN 'Failed after fault (Deviated)'
        ELSE 'Both passed (Completed)'
    END AS hypothesis_result,
    COUNT(*) AS count,
    ROUND(COUNT(*) * 100.0 / SUM(COUNT(*)) OVER (), 1) AS pct
FROM experiments
GROUP BY hypothesis_result
ORDER BY count DESC;
```

A high "Failed before fault" rate indicates systemic instability — your systems are frequently unhealthy before the experiment even starts. This is valuable data that pure uptime monitoring would not surface.

---

## Exporting to Parquet

Parquet export makes experiment data portable to any data tool:

```bash
# Export a single journal
tumult export journal.toon --format parquet

# Export to CSV (for spreadsheets)
tumult export journal.toon --format csv

# Export to JSON (for compatibility with other tools)
tumult export journal.toon --format json
```

A Parquet file from Tumult loads directly into pandas:

```python
import pandas as pd

df = pd.read_parquet("journal.parquet")
print(df[["title", "status", "resilience_score", "duration_ms"]].head(20))
```

Or into Polars for faster processing:

```python
import polars as pl

df = pl.read_parquet("journals/*.parquet")
print(df.group_by("status").agg(pl.count(), pl.col("resilience_score").mean()))
```

Or directly into a DuckDB session for ad-hoc SQL:

```python
import duckdb

conn = duckdb.connect()
result = conn.execute("""
    SELECT title, avg(resilience_score) as score
    FROM read_parquet('journals/*.parquet')
    GROUP BY title
    ORDER BY score ASC
""").fetchdf()
```

---

## Why Embedded DuckDB?

The choice to embed DuckDB rather than require an external database is deliberate.

**No infrastructure.** The analytics capability is part of the `tumult` binary. There is no separate database process, no network connection, no credentials to manage. It works the same on a developer laptop and in a CI container.

**Columnar performance.** DuckDB is an analytical database — optimized for aggregation queries over many rows. The query patterns in chaos analytics (group by experiment name, aggregate over time, compute percentiles) are exactly what columnar databases excel at.

**Parquet-native.** DuckDB reads Parquet files directly. Your Tumult journals stored as Parquet in S3 are immediately queryable with `read_parquet('s3://your-bucket/journals/*.parquet')` without importing or transforming data.

**Arrow interoperability.** Tumult's internal analytics builds on Apache Arrow RecordBatches. The Arrow ecosystem — including Polars, pandas, PyArrow, DataFusion, and dozens of other tools — can all consume this data format without conversion.

---

## The Bigger Picture: Chaos Data as Organizational Memory

Individual experiments answer specific questions about specific systems at specific times. Analytics across all experiments answer organizational questions: what is the overall trajectory of system resilience? Which teams are running experiments? Which critical systems have never been chaos-tested?

The Parquet pipeline is the foundation for this. Every experiment produces a structured, portable record. Over time, those records accumulate into an organizational data asset: a queryable history of resilience testing, failure modes, recovery times, and prediction accuracy.

That data asset is what enables the shift from chaos engineering as occasional practice to chaos engineering as continuous discipline. Instead of "we ran some experiments last quarter," you have "here is the resilience trend for every critical service over the past year, with statistical baselines for each."

The trend query — is recovery time improving, stable, or degrading? — is the one that matters most:

```sql
WITH ranked AS (
    SELECT
        title,
        started_at_ns,
        duration_ms,
        LAG(duration_ms) OVER (PARTITION BY title ORDER BY started_at_ns) AS prev_ms
    FROM experiments
    WHERE status IN ('Completed', 'Deviated')
)
SELECT
    title,
    COUNT(*) AS runs,
    AVG(duration_ms - prev_ms) AS avg_delta_ms,
    CASE
        WHEN AVG(duration_ms - prev_ms) > 5000 THEN 'DEGRADING'
        WHEN AVG(duration_ms - prev_ms) < -5000 THEN 'IMPROVING'
        ELSE 'STABLE'
    END AS trend
FROM ranked
WHERE prev_ms IS NOT NULL
GROUP BY title
ORDER BY avg_delta_ms DESC;
```

Services trending `DEGRADING` need attention. Services trending `IMPROVING` are demonstrating the value of the engineering work invested in resilience.

---

*Next in the series: [Part 7 — Kubernetes Chaos: Deep Fault Injection with tumult-kubernetes →](./07-kubernetes-chaos.md)*
