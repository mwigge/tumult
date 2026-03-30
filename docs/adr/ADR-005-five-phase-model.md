---
title: "ADR-005: Five-Phase Model"
parent: Architecture Decisions
nav_order: 5
---

# ADR-005: Five-Phase Data Model with Statistical Baseline Methods

**Status:** Accepted
**Date:** 2026-03-29
**Decision Makers:** mwigge

## Context

Traditional chaos engineering tools follow a two-phase model: check steady state before fault injection, inject the fault, check steady state after. This misses the degradation curve during fault injection (how the system degrades, not just whether it recovered), provides no mechanism for prediction tracking (did the team expect the right outcome?), and produces insufficient evidence for regulatory compliance frameworks like DORA, NIS2, and PCI-DSS that require documented resilience validation with measurable baselines.

Furthermore, traditional tools use static thresholds for steady-state hypothesis (e.g., "HTTP status must be 200", "latency must be < 500ms"). These thresholds are guessed, not measured, and become stale after deployments or load changes. The platform needs a scientific approach: measure the system first, derive thresholds from data, then evaluate deviations statistically.

## Decision

### Five-Phase Data Model

Adopt a five-phase data model for experiment execution:

1. **Estimate** -- Team records predictions before execution: expected impact, expected recovery time, expected degradation pattern. Tracked for calibration over time.
2. **Baseline** -- Statistical measurement of steady-state behavior before fault injection. Can be static (fixed thresholds), statistical (mean plus/minus N standard deviations from sampled data), or learned (adaptive query execution). Produces quantified baselines, not just pass/fail checks.
3. **During** -- Continuous probe sampling during fault injection captures the full degradation curve: how fast the system degrades, whether it oscillates, and at what level it stabilizes.
4. **Post** -- Recovery measurement after fault removal: time to recovery, recovery completeness, whether the system returns to baseline or settles at a new steady state.
5. **Analysis** -- Cross-run learning: compare this execution against historical runs, update prediction calibration scores, generate regulatory evidence artifacts, feed resilience scoring models.

### Statistical Methods for Baseline Derivation

Four statistical methods for baseline derivation, selectable per experiment:

| Method | Formula | Best For |
|--------|---------|----------|
| **Static** | Fixed lower/upper bounds | Simple binary checks (up/down) |
| **Mean +/- N-sigma** | mu +/- N-sigma (configurable N, typically 2) | Normally distributed metrics (throughput, connections) |
| **Percentile** | p(N) x multiplier | Latency SLOs (skewed distributions) |
| **IQR** | Q1 - 1.5xIQR to Q3 + 1.5xIQR | Noisy environments, outlier-robust |

Additionally:
- Anomaly detection on the baseline itself (coefficient of variation > 0.5, extreme range, minimum samples)
- Recovery detection via backward scan for last threshold breach
- Compliance ratio (proportion of post-fault samples within tolerance)

## Consequences

### Positive

**Five-phase model:**
- Scientific rigor: captures the full degradation and recovery curve, not just before/after snapshots
- Prediction tracking enables team learning and calibration measurement over time
- Regulatory evidence chain: each phase produces auditable artifacts suitable for DORA, NIS2, and PCI-DSS compliance
- Baselined thresholds are self-calibrating -- statistical baselines adapt to system evolution without manual threshold updates
- During-phase data enables degradation pattern classification (graceful, cliff, oscillating, cascading)

**Statistical baseline methods:**
- Self-calibrating: thresholds adapt to the system's actual behavior
- Scientifically grounded: each method has known statistical properties
- Auditable: raw samples preserved, derivation method recorded in journal

### Negative

**Five-phase model:**
- More complex execution flow compared to simple before/after models
- Longer experiment duration due to baseline sampling phase (statistical baseline requires multiple samples)
- More data to store and process per experiment run
- Higher barrier to entry for simple experiments that do not need all five phases

**Statistical baseline methods:**
- Requires a baseline phase (adds experiment duration)
- Statistical methods assume stationarity during baseline window
- Small sample sizes produce unreliable bounds

### Risks
- Baseline phase duration must be configurable; overly long baselines will discourage adoption for quick validation runs
- Analysis phase depends on historical data availability; first runs produce limited cross-run insights
- The five-phase model may be over-engineered for simple smoke-test experiments; the engine should support graceful phase skipping (e.g., skip Estimate for automated runs)
- A degraded baseline (system already stressed) produces permissive thresholds. Mitigated by anomaly detection.
