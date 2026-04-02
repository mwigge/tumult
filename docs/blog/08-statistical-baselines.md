---
title: "Statistical Baselines: From Magic Numbers to Data-Derived Tolerances"
parent: Blog
nav_order: 8
---

# <img src="/images/tumult.png" alt="Tumult Logo" width="100" valign="middle"> Statistical Baselines: From Magic Numbers to Data-Derived Tolerances

![Tumult Banner](/images/tumult-banner.png)

*Part 8 of the Tumult series. [← Part 7: Kubernetes Chaos](./07-kubernetes-chaos.md)*

---

Every chaos experiment has a steady-state hypothesis: a set of probes that define what "healthy" looks like, with tolerances that define the acceptable range. Get the tolerances wrong — too tight and experiments fail spuriously; too loose and real degradation goes undetected.

Most chaos tools require you to set these tolerances manually. You guess. You pick "response time under 500ms" because it sounds reasonable, or because it is what the SLA says. But the SLA latency target and the actual baseline latency of the system are two different things. If your service normally runs at 45ms, a 500ms tolerance will not catch the moment when it spikes to 300ms after a database connection kill.

Tumult's baseline engine replaces guesses with measurements.

---

## The Problem With Static Thresholds

Static tolerances look like this:

```toon
tolerance:
  type: range
  from: 0
  to: 500    # "latency must be under 500ms"
```

They work until they do not. Problems:

1. **They are coupled to the SLA, not the system.** Your SLA says 500ms. Your system runs at 45ms. The tolerance is not testing whether the system degraded — it is testing whether it crossed the SLA boundary. Those are different questions.

2. **They are environment-specific in ways that are hard to manage.** The staging environment runs at 120ms (more load, less hardware). The production environment runs at 45ms. A static tolerance valid for one is wrong for the other.

3. **They do not account for normal variance.** Some metrics have high natural variance. A tight static threshold will generate false alarms. A loose threshold will miss real degradation.

The baseline engine solves all three problems by measuring the system before fault injection and deriving tolerances from the measured data.

---

## How Baseline Acquisition Works

When a Tumult experiment has a `baseline` section configured, the engine runs Phase 1 before fault injection:

```
1. Connect to all probe targets
2. Warmup — collect and discard initial samples (settling time)
3. Sample — collect values at configured interval for configured duration
4. Compute statistics: mean, stddev, p50, p90, p95, p99, min, max
5. Detect anomalies in the baseline itself
6. Derive tolerance bounds using the configured method
7. Update the steady-state hypothesis with derived tolerances
8. Proceed to fault injection
```

The derived tolerances replace (or augment) the static tolerances in the hypothesis for this run. The baseline statistics are recorded in the journal as Phase 1 evidence.

---

## The Four Baseline Methods

### 1. Static (pass-through)

When you want the traditional behavior: fixed thresholds, no measurement.

```toon
baseline:
  method: static
  tolerance_lower: 0
  tolerance_upper: 500
```

Use for: binary health checks (up/down), known-exact expected values, compatibility with traditional experiments.

---

### 2. Mean ± Nσ (mean_stddev)

The most common method. Measures the arithmetic mean and standard deviation, then sets the tolerance at `mean ± N × stddev`.

```toon
baseline:
  method: mean_stddev
  duration_s: 120.0
  warmup_s: 15.0
  interval_s: 2.0
  sigma: 2.0       # 2 standard deviations ≈ 95% of normal values
  confidence: 0.95
```

**What this gives you**: if your service normally runs at 45ms with a stddev of 5ms, the derived tolerance is `45 ± 2×5 = [35ms, 55ms]`. Any post-fault measurement outside that range is a deviation.

**When to use**: throughput metrics (requests/second), connection counts, error rates — metrics that are approximately normally distributed.

**σ selection guide**:
- σ=1.5 → 86% of normal values in bounds (tighter, more sensitive)
- σ=2.0 → 95% of normal values in bounds (standard choice)
- σ=2.5 → 99% of normal values in bounds (permissive, use for noisy metrics)
- σ=3.0 → 99.7% of normal values in bounds (very permissive)

---

### 3. Percentile

Uses a percentile value with a safety multiplier. Designed for latency metrics, which are right-skewed: the tail is much longer than the body of the distribution.

```toon
baseline:
  method: percentile
  duration_s: 120.0
  interval_s: 2.0
  percentile: 95     # derive the threshold from p95
  multiplier: 1.2    # allow 20% headroom above p95
```

**What this gives you**: if p95 latency during baseline is 52ms, the derived upper tolerance is `52 × 1.2 = 62.4ms`. The probe will flag any post-fault measurement above that.

**When to use**: latency (p50, p95, p99), response time distributions, any right-skewed metric where mean ± stddev does not capture the tail behavior.

**Why not use mean for latency**: latency distributions are notoriously right-skewed. A mean-based tolerance will miss latency spikes that only affect the tail. The p95 represents "what 95% of real users experience" — a much more meaningful basis for a tolerance.

---

### 4. IQR (Interquartile Range)

Robust to outliers. Uses Q1 - 1.5×IQR to Q3 + 1.5×IQR (Tukey's fences). Works well in noisy environments where occasional outliers should not widen the tolerance bounds.

```toon
baseline:
  method: iqr
  duration_s: 120.0
  interval_s: 2.0
```

**What this gives you**: the "normal" range excluding outliers. If your metric has occasional spikes unrelated to the experiment (cron jobs, GC pauses), IQR ignores them when setting the baseline.

**When to use**: noisy environments, metrics with frequent outliers, anywhere mean ± stddev is too sensitive to outlier pollution.

---

## Method Selection Guide

| Metric | Method | Why |
|--------|--------|-----|
| HTTP response time (p50, p95, p99) | Percentile | Skewed distribution; tail matters |
| Throughput (requests/second) | Mean ± Nσ | Approximately normal |
| Error rate (0.0–1.0) | Mean ± Nσ (σ=2) | Bounded, approximately normal |
| Database connections | Mean ± Nσ | Count, approximately normal |
| Kafka consumer lag | Percentile | Skewed; spikes are meaningful |
| CPU utilization | IQR | Noisy; resistant to outliers |
| Binary health check | Static (exact) | Not a distribution — it's 0 or 1 |
| Cold-start metrics | Percentile + warmup | Discard settling period |

---

## Warmup: Discarding Settlement Noise

Many systems need time to stabilize after connections are established. Connection pools initialize, caches warm up, DNS entries propagate. The first samples after a connection is established reflect initialization, not steady-state.

The `warmup_s` parameter discards early samples:

```toon
baseline:
  warmup_s: 15.0   # discard first 15 seconds
  duration_s: 120.0  # then collect 120 seconds of data
```

With `warmup_s: 15` and `interval_s: 2`, the first 7-8 samples are discarded. The statistical baseline is computed from the remaining samples only.

This is especially important for:
- Cold-start systems that JIT-compile on first use
- Connection-pooled databases that initialize on first query
- CDN-backed services where DNS changes propagate gradually

---

## Anomaly Detection: Don't Run Chaos on a Sick System

Before deriving thresholds, the baseline engine checks if the baseline itself is anomalous:

```
High variance:      coefficient_of_variation > 0.5 (50%)
Extreme range:      max - min > 10 × median
Insufficient data:  fewer than minimum required samples
```

If any of these conditions are true, the engine warns and optionally aborts:

```
Warning: baseline anomaly detected for probe 'latency-probe'
  method: mean_stddev
  coefficient_of_variation: 0.73 (threshold: 0.50)
  This may indicate the system is already degraded or experiencing
  high variability. Results may not be meaningful.
  
  To abort: set baseline.anomaly_abort: true
  To continue with warning: set baseline.anomaly_abort: false (default)
```

Running a chaos experiment on an already-degraded system produces meaningless results. The baseline anomaly check is a guardrail against this mistake.

---

## Baseline in the Journal: Phase 1 Evidence

The derived baseline is recorded in the journal as Phase 1 evidence. Here is what it looks like for a latency probe with mean_stddev method:

```toon
baseline:
  method: mean_stddev
  started_at_ns: 1705327391000000000
  ended_at_ns: 1705327511000000000
  duration_s: 120.0
  samples: 57
  warmup_samples: 7

  probes:
    latency-probe:
      mean: 45.2
      stddev: 3.8
      p50: 44.0
      p95: 52.3
      p99: 58.7
      min: 38.1
      max: 61.4
      error_rate: 0.0
      derived_lower: 37.6      # mean - 2σ
      derived_upper: 52.8      # mean + 2σ
      anomaly_detected: false
```

This is permanent evidence in the journal. An auditor or engineer can see exactly what the system's normal operating parameters were on the day the experiment ran, and verify that the tolerances were derived from measured data rather than guessed.

---

## A Complete Experiment With Dynamic Baseline

Here is a production-grade experiment that uses the mean_stddev baseline for a database query latency probe:

```toon
title: Database primary kill validates connection failover
description: |
  Measure database query latency baseline, kill primary connections,
  and verify recovery within derived tolerance bounds.

tags[3]: database, postgresql, resilience

estimate:
  expected_outcome: recovered
  expected_recovery_s: 20.0
  expected_degradation: severe
  expected_data_loss: false
  confidence: high

baseline:
  duration_s: 120.0
  warmup_s: 15.0
  interval_s: 2.0
  method: mean_stddev
  sigma: 2.0
  confidence: 0.95
  anomaly_abort: true     # abort if baseline is anomalous

steady_state_hypothesis:
  title: Database query latency within baseline tolerance
  probes[1]:
    - name: query-latency
      activity_type: probe
      provider:
        type: process
        path: plugins/tumult-db-postgres/probes/query-latency.sh
        env:
          TUMULT_DB_HOST: "{{ configuration.db_host }}"
          TUMULT_QUERY: "SELECT 1"
      # No static tolerance — will be replaced by derived bounds from baseline
      tolerance:
        type: range
        from: 0
        to: 9999     # placeholder — overridden by baseline engine

method[1]:
  - name: kill-primary-connections
    activity_type: action
    provider:
      type: native
      plugin: tumult-db
      function: terminate_connections
      arguments:
        database: payments
    pause_after_s: 5.0

rollbacks[1]:
  - name: reset-connection-pool
    activity_type: action
    provider:
      type: native
      plugin: tumult-db
      function: reset_connection_pool
```

After this experiment runs, the journal contains:
- The baseline-derived tolerance for `query-latency` (e.g., `[38ms, 53ms]`)
- The during-fault peak value (e.g., `2847ms` — the timeout period during reconnection)
- The post-fault recovery time (how long until query latency returned within the derived bounds)

These are meaningful, reproducible results. The tolerance is not a guess — it is derived from the system's actual behavior on the day the experiment ran.

---

## Baseline Mode Options

When running experiments, you can control baseline behavior:

```bash
# Full pipeline — acquire baseline, inject fault, measure recovery
tumult run experiment.toon --baseline-mode full

# Skip baseline — use static tolerances from the experiment definition
tumult run experiment.toon --baseline-mode skip

# Baseline only — measure and report without injecting faults
tumult run experiment.toon --baseline-mode only
```

`--baseline-mode only` is useful for getting a snapshot of current system behavior without any risk of fault injection. Use it to verify the experiment configuration and understand what the derived tolerances would be before running the full experiment.

---

*Next in the series: [Part 9 — Compliance as Code: DORA, NIS2, and Regulatory Evidence with Tumult →](./09-regulatory-compliance.md)*
