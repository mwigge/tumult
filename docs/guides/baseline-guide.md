---
title: Statistical Baselines
parent: Guides
nav_order: 4
---

# Baseline Guide

Tumult's baseline engine replaces static thresholds with data-driven tolerance derivation. Instead of guessing "latency must be < 500ms", the engine measures the system and derives "latency should stay within 2 standard deviations of the measured 45ms mean."

The following statistical methods belong to the `tumult-baseline` library.
The normal experiment runner does not acquire a statistical baseline or apply
these configuration snippets automatically. `tumult run --baseline-mode only`
evaluates the declared probes once without faults; `full` and `skip` use
static declared tolerances. See [Data Lifecycle](../data-lifecycle.md) for
current runner behavior. A custom integration must collect numeric samples,
invoke the library, and explicitly apply derived tolerances.

## Baseline Methods

### Static

Fixed thresholds — compatible with traditional chaos tools.

```toon
baseline:
  method: static
  tolerance_lower: 0
  tolerance_upper: 500
```

### Mean ± Nσ (Mean Standard Deviation)

Derives bounds from the arithmetic mean plus/minus N standard deviations. Best for normally distributed metrics like throughput and connection counts.

```toon
baseline:
  method: mean_stddev
  duration_s: 120
  interval_s: 2
  sigma: 2.0
```

With σ=2, approximately 95% of normal values fall within the bounds.

### Percentile

Uses a percentile value with a safety multiplier. Best for latency metrics which are typically right-skewed.

```toon
baseline:
  method: percentile
  duration_s: 120
  interval_s: 2
  percentile: 95
  multiplier: 1.2
```

The threshold is p95 × 1.2, giving 20% headroom above the observed 95th percentile.

### IQR (Interquartile Range)

Robust to outliers. Uses Q1 - 1.5×IQR to Q3 + 1.5×IQR. Best for noisy environments.

```toon
baseline:
  method: iqr
  duration_s: 120
  interval_s: 2
```

## Baseline Phases

### Warmup

The first N seconds of baseline collection are discarded. This accounts for system settling time (cold caches, connection pool initialization).

```toon
baseline:
  warmup_s: 15
  duration_s: 120
```

### Anomaly Detection

Before deriving thresholds, the engine checks if the baseline data itself is anomalous:

- **High variance**: coefficient of variation > 0.5 (50%)
- **Extreme range**: max - min > 10× the median
- **Insufficient samples**: fewer than the minimum required

The library reports anomalies to its caller. A custom integration must decide
whether to abort or continue and record that decision; ordinary experiment
execution does not automatically perform this statistical gate.

The anomaly check emits a `anomaly.detected` span event on the `baseline.acquire` span, visible in your OTel backend. The span status is set to `ERROR` when an anomaly is found.

### Recovery Detection

After fault removal, the engine scans post-fault samples to find the recovery point — the first index where all subsequent samples are within tolerance. This gives the MTTR (Mean Time to Recovery).

### Compliance Ratio

The proportion of post-fault samples within tolerance bounds. A ratio of 1.0 means full recovery; 0.5 means half the post-fault samples breached the baseline.

## Streaming Acquisition (`AcquisitionStream`)

For advanced use cases (custom probes, incremental data feeds), `tumult-baseline` exposes `AcquisitionStream` — a streaming interface that accepts samples one at a time rather than collecting a full dataset upfront:

```rust
use tumult_baseline::{AcquisitionStream, AcquisitionConfig, Method};

let config = AcquisitionConfig {
    method: Method::MeanStddev { sigma: 2.0 },
    min_samples: 3,
};

let mut stream = AcquisitionStream::new("api-latency".into(), config);

// Feed samples incrementally (e.g., from a live probe loop)
stream.push_sample(45.2);
stream.push_sample(47.8);
stream.push_sample(43.1);

// Derive tolerance bounds at any point
if let Ok(result) = stream.derive() {
    println!("lower={}, upper={}", result.tolerance_lower, result.tolerance_upper);
}
```

This stream is available to custom integrations; the ordinary runner does not
wire it into live baseline capture.

## Choosing a Method

| Metric Type | Recommended Method | Why |
|-------------|-------------------|-----|
| Latency (p50, p95, p99) | Percentile | Latency distributions are skewed |
| Throughput (req/s) | Mean ± Nσ | Throughput is approximately normal |
| Error rate | Mean ± Nσ with σ=2 | Error rates are bounded 0-1 |
| Connection count | Mean ± Nσ | Counts are approximately normal |
| Binary health check | Static (0 or 1) | No distribution to measure |
| Noisy metrics | IQR | Robust to outliers |

## Property-Based Testing

The statistical functions in `tumult-baseline` are covered by property-based tests using `proptest`. These tests generate arbitrary datasets and verify mathematical invariants (e.g., mean is always between min and max, stddev is non-negative) rather than relying on fixed examples. Run them with:

```bash
cargo test -p tumult-baseline
```
