# ADR-009: Statistical Methods for Baseline Derivation

**Status:** Accepted
**Date:** 2026-03-29
**Decision Makers:** mwigge

## Context

Traditional chaos engineering tools use static thresholds for steady-state hypothesis (e.g., "HTTP status must be 200", "latency must be < 500ms"). These thresholds are guessed, not measured, and become stale after deployments or load changes.

Tumult needs a scientific approach: measure the system first, derive thresholds from data, then evaluate deviations statistically.

## Decision

Four statistical methods for baseline derivation, selectable per experiment:

| Method | Formula | Best For |
|--------|---------|----------|
| **Static** | Fixed lower/upper bounds | Simple binary checks (up/down) |
| **Mean ± Nσ** | μ ± Nσ (configurable N, typically 2) | Normally distributed metrics (throughput, connections) |
| **Percentile** | p(N) × multiplier | Latency SLOs (skewed distributions) |
| **IQR** | Q1 - 1.5×IQR to Q3 + 1.5×IQR | Noisy environments, outlier-robust |

Additionally:
- Anomaly detection on the baseline itself (coefficient of variation > 0.5, extreme range, minimum samples)
- Recovery detection via backward scan for last threshold breach
- Compliance ratio (proportion of post-fault samples within tolerance)

## Consequences

### Positive

- Self-calibrating: thresholds adapt to the system's actual behavior
- Scientifically grounded: each method has known statistical properties
- Auditable: raw samples preserved, derivation method recorded in journal

### Negative

- Requires a baseline phase (adds experiment duration)
- Statistical methods assume stationarity during baseline window
- Small sample sizes produce unreliable bounds

### Risks

- A degraded baseline (system already stressed) produces permissive thresholds. Mitigated by anomaly detection.
