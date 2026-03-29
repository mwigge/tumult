# Tumult Data Lifecycle

Five-phase data lifecycle for resilience testing experiments. Every experiment progresses through these phases sequentially. The journal captures all five phases as structured evidence.

---

## Phase Overview

```
  Phase 0        Phase 1         Phase 2          Phase 3         Phase 4
  ESTIMATE       BASELINE        DURING           POST            ANALYSIS
  ─────────┬─────────────┬──────────────┬──────────────┬──────────────────
           │             │              │              │
  Operator │  Measure    │  Fault       │  Recovery    │  Cross-run
  predicts │  before     │  active      │  after       │  learning
  outcome  │  fault      │              │  rollback    │
           │             │              │              │
```

---

## Phase 0 — ESTIMATE (prediction)

The operator declares the expected outcome BEFORE any measurement occurs. This is not optional decoration — comparing prediction vs actual is how teams learn. An experiment without an estimate is a measurement without a hypothesis.

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `expected_outcome` | `string` | One of: `no_impact`, `degraded`, `partial_outage`, `full_outage` |
| `expected_recovery_s` | `f64` | Predicted recovery time in seconds |
| `expected_degradation` | `f64` | Predicted degradation magnitude (0.0 to 1.0) |
| `expected_data_loss` | `bool` | Whether data loss is expected |
| `confidence` | `f64` | Operator confidence in prediction (0.0 to 1.0) |
| `rationale` | `string` | Free-text explanation of why this outcome is expected |

### Attributes

```
resilience.estimate.expected_outcome     = "degraded"
resilience.estimate.expected_recovery_s  = 30.0
resilience.estimate.expected_degradation = 0.15
resilience.estimate.expected_data_loss   = false
resilience.estimate.confidence           = 0.7
resilience.estimate.rationale            = "Kafka replication factor 3, killing 1 broker should cause brief consumer rebalance"
```

### AQE tracking

Over many runs, the AQE (Agentic QE) fleet can track prediction accuracy per team, per system, per failure mode. Teams that consistently over-estimate resilience are learning something different from teams that under-estimate it. Both patterns are valuable signals.

---

## Phase 1 — BASELINE (measurement before fault)

Connect to all probe targets and establish what "normal" looks like before injecting any fault. The baseline is the ruler against which all subsequent phases are measured.

### Procedure

1. **Connect** to all probe targets (SSH, HTTP, database, Kafka JMX, etc.)
2. **Warmup** — discard the first N samples (settling time after connection establishment)
3. **Sample** at the configured interval for the configured duration
4. **Derive** statistical baseline from the collected samples
5. **Detect** if the baseline itself is anomalous (system already degraded before fault injection)

### Statistical Methods

| Method | Formula | When to use |
|--------|---------|-------------|
| `percentile` | p50, p90, p95, p99 of samples | Latency measurements |
| `mean-stddev` | mean +/- N * stddev (default N=2) | Normally distributed metrics |
| `iqr` | Q1 - 1.5*IQR to Q3 + 1.5*IQR | Skewed distributions, outlier-resistant |
| `error-rate` | errors / total requests | HTTP error rates, query failure rates |
| `availability` | uptime / (uptime + downtime) | Service availability probes |

### Baseline Output

```
resilience.baseline.method           = "mean-stddev"
resilience.baseline.samples          = 120
resilience.baseline.interval_s       = 1.0
resilience.baseline.warmup_samples   = 10
resilience.baseline.mean             = 45.2
resilience.baseline.stddev           = 3.8
resilience.baseline.p50              = 44.0
resilience.baseline.p90              = 50.1
resilience.baseline.p95              = 52.3
resilience.baseline.p99              = 58.7
resilience.baseline.iqr              = 5.2
resilience.baseline.anomalous        = false
```

### Anomaly detection

If the baseline itself is anomalous (e.g., error rate already elevated, latency already spiking), the experiment should emit a warning and optionally abort. Running a chaos experiment on an already-degraded system produces meaningless results.

---

## Phase 2 — DURING (observation under fault)

Continuous sampling while the fault is active. This phase captures the degradation curve — the shape of the system's response to the injected fault.

### Procedure

1. **Continue sampling** with the same probes and interval as baseline
2. **Detect onset** — the timestamp when metrics first breach the baseline threshold
3. **Track peak** — the maximum deviation from baseline
4. **Classify shape** — graceful degradation (curve) vs catastrophic failure (cliff)
5. **Record threshold breaches** in real-time as OTel events on the active span

### The Degradation Curve

```
 Response
 Time (ms)
    ^
    |
200 |                          * *
    |                        *     *        <-- peak degradation
180 |                      *         *
    |                    *             *
160 |                  *                 *
    |                *                     *
140 |              *                         *
    |           *                               *
120 |         *                                   *
    |       *                                       * * * * *
100 |  * * *                                                    * * * *
    |  baseline                                                 recovered
 80 |
    +-----+--------+-----------+----------+-----------+-----------> time
          |        |           |          |           |
       baseline  onset      peak      rollback    recovered
        ends    detected   reached    initiated
```

### Graceful vs Catastrophic

```
 GRACEFUL (curve)                    CATASTROPHIC (cliff)

    ^                                    ^
    |       . * * .                      |
    |     .         .                    |          * * * * * * *
    |    .            .                  |          |
    |  .                .                |          |
    | .                   .              |          |
    |.                      .            |  * * * * |
    +------------------------->          +------------------------->
```

### Attributes

```
resilience.during.onset_epoch_ns     = 1711234567890123456
resilience.during.peak_epoch_ns      = 1711234578901234567
resilience.during.peak_value          = 198.4
resilience.during.peak_deviation      = 153.2          # absolute deviation from baseline mean
resilience.during.peak_deviation_pct  = 338.9          # percentage deviation
resilience.during.shape               = "graceful"     # or "catastrophic"
resilience.during.threshold_breaches  = 47
resilience.during.samples             = 60
```

All data is streamed as OTel metrics via OTLP. The collector routes to storage.

---

## Phase 3 — POST (recovery measurement)

Measurement after the fault has been removed or rolled back. Uses the same probes, same interval, and same duration as the baseline phase — the two phases must be directly comparable.

### Procedure

1. **Roll back** or remove the fault (automated by the experiment's rollback steps)
2. **Continue sampling** with identical probe configuration as Phase 1
3. **Track recovery time** per probe — the duration from rollback initiation to metric returning within baseline thresholds
4. **Verify data integrity** — for stateful systems, confirm no data loss or corruption
5. **Calculate MTTR** — Mean Time To Recovery across all probes

### Recovery Detection

A probe is "recovered" when its value returns to within the baseline threshold for a sustained period (configurable, default: 10 consecutive samples within threshold).

### Attributes

```
resilience.post.recovery_epoch_ns         = 1711234600123456789
resilience.post.recovery_duration_s       = 32.5
resilience.post.mttr_s                    = 32.5
resilience.post.data_integrity_verified   = true
resilience.post.data_loss_detected        = false
resilience.post.fully_recovered           = true
resilience.post.recovery_samples          = 120
resilience.post.samples_within_threshold  = 118
```

---

## Phase 4 — ANALYSIS (cross-run learning)

Post-experiment analysis that spans multiple experiment runs. This is where individual experiments become organizational knowledge.

### Capabilities

1. **Estimate vs Actual** — compare Phase 0 predictions with Phase 2/3 observations
2. **Trend detection** — track recovery time, degradation magnitude, and prediction accuracy across runs
3. **Resilience scoring** — compute a composite resilience score from the experiment evidence
4. **Regulatory evidence generation** — produce audit-ready reports mapping experiment results to DORA, NIS2, PCI-DSS requirements (see `docs/regulatory-mapping.md`)
5. **AQE pattern learning** (Phase 3 of the project) — the AQE fleet learns which failure modes produce unexpected results and adjusts experiment selection

### Attributes

```
resilience.analysis.estimate_accuracy     = 0.82
resilience.analysis.estimate_outcome_match = false    # predicted no_impact, observed degraded
resilience.analysis.trend_direction        = "improving"
resilience.analysis.trend_run_count        = 15
resilience.analysis.resilience_score       = 0.76
resilience.analysis.regulatory_frameworks  = "DORA,NIS2,PCI-DSS"
```

---

## Time Standard

All timestamps in the Tumult data model use a consistent convention:

| Quantity | Type | Unit | Example |
|----------|------|------|---------|
| Point in time | `i64` | Epoch nanoseconds | `1711234567890123456` |
| Duration | `f64` | Seconds | `32.5` |

Epoch nanoseconds provide sub-microsecond precision and align with OTel's timestamp format. Seconds for durations provide human readability while retaining millisecond precision in the fractional part.

---

## Data Flow

```
 experiment.toon                          OTel Collector
 (experiment                              (fan-out)
  definition)                                 │
      │                                       ├──> Jaeger (traces)
      ▼                                       ├──> Prometheus (metrics)
 ┌──────────┐    OTLP (gRPC/HTTP)            ├──> Loki (logs)
 │  tumult   │ ──────────────────────────────>│
 │  engine   │                                └──> DuckDB/Parquet
 │           │                                     (journals +
 │           │                                      analytics)
 │           │──── journal.toon
 │           │     (structured experiment
 └──────────┘      output in TOON format)
```

### Integration Principle

The OTel Collector is THE integration point. Tumult speaks OTLP only — it does not integrate directly with Jaeger, Prometheus, Loki, or any other backend. The Collector receives OTLP and routes (fans out) to all configured backends.

This means:

- Tumult has exactly one export dependency: OTLP
- Adding a new backend (Grafana Tempo, Elastic APM, Datadog) is a Collector config change, not a Tumult code change
- The Collector handles sampling, batching, retry, and back-pressure

### Local Analytics: DuckDB + Parquet

For local analysis without a full observability stack, journals are stored in DuckDB (embedded, zero-dependency) and exported as Parquet files for portability.

```
journal.toon ──> tumult-report ──> DuckDB (embedded)
                                       │
                                       ├──> SQL queries (interactive)
                                       └──> Parquet export (share/archive)
```

DuckDB is chosen because:
- Embedded — no server process, no network, no setup
- Columnar — efficient for analytical queries over experiment metrics
- Parquet-native — reads and writes Parquet directly
- SQL — familiar query language for ad-hoc analysis

---

## Example SQL Queries Against Journals

### Recovery time trend over last 30 days

```sql
SELECT
    experiment_title,
    DATE_TRUNC('day', started_at) AS run_date,
    AVG(recovery_duration_s) AS avg_recovery_s,
    MIN(recovery_duration_s) AS best_recovery_s,
    MAX(recovery_duration_s) AS worst_recovery_s,
    COUNT(*) AS run_count
FROM journals
WHERE started_at > CURRENT_TIMESTAMP - INTERVAL '30 days'
GROUP BY experiment_title, run_date
ORDER BY experiment_title, run_date;
```

### Estimate accuracy by team

```sql
SELECT
    tags->>'team' AS team,
    COUNT(*) AS experiments,
    AVG(CASE WHEN estimate_outcome = actual_outcome THEN 1.0 ELSE 0.0 END) AS outcome_accuracy,
    AVG(ABS(estimate_recovery_s - actual_recovery_s)) AS avg_recovery_error_s,
    AVG(estimate_confidence) AS avg_confidence
FROM journals
WHERE estimate_outcome IS NOT NULL
GROUP BY tags->>'team'
ORDER BY outcome_accuracy DESC;
```

### Systems with degrading resilience (recovery time trending upward)

```sql
WITH ranked AS (
    SELECT
        target_system,
        started_at,
        recovery_duration_s,
        LAG(recovery_duration_s) OVER (
            PARTITION BY target_system ORDER BY started_at
        ) AS prev_recovery_s
    FROM journals
    WHERE status = 'completed' AND recovery_duration_s IS NOT NULL
)
SELECT
    target_system,
    COUNT(*) AS runs,
    AVG(recovery_duration_s - prev_recovery_s) AS avg_delta_s,
    CASE
        WHEN AVG(recovery_duration_s - prev_recovery_s) > 5.0 THEN 'DEGRADING'
        WHEN AVG(recovery_duration_s - prev_recovery_s) < -5.0 THEN 'IMPROVING'
        ELSE 'STABLE'
    END AS trend
FROM ranked
WHERE prev_recovery_s IS NOT NULL
GROUP BY target_system
ORDER BY avg_delta_s DESC;
```

### Experiments with worst estimate-vs-actual deviation

```sql
SELECT
    experiment_title,
    started_at,
    estimate_outcome,
    actual_outcome,
    estimate_recovery_s,
    recovery_duration_s AS actual_recovery_s,
    ABS(estimate_recovery_s - recovery_duration_s) AS recovery_error_s,
    estimate_confidence
FROM journals
WHERE estimate_outcome IS NOT NULL
    AND estimate_outcome != actual_outcome
ORDER BY recovery_error_s DESC
LIMIT 20;
```

---

## Attribute Namespace Summary

All resilience testing attributes live under the `resilience.*` namespace:

| Prefix | Phase | Purpose |
|--------|-------|---------|
| `resilience.estimate.*` | 0 | Operator predictions before experiment |
| `resilience.baseline.*` | 1 | Statistical baseline before fault |
| `resilience.during.*` | 2 | Observations under active fault |
| `resilience.post.*` | 3 | Recovery measurements after rollback |
| `resilience.analysis.*` | 4 | Cross-run analysis and scoring |

The `tumult.*` namespace is reserved for engine-level instrumentation (see the design doc for the full attribute list). The `resilience.*` namespace is the domain-level data model for the experiment lifecycle.
