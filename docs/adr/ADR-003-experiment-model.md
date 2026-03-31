---
title: "ADR-003: Experiment Model"
parent: Architecture Decisions
nav_order: 3
---

# ADR-003: Five-Phase Experiment Model with Load Integration

## Status

Accepted

## Context

Traditional chaos tools use a simple hypothesis-method-rollback model. Modern resilience testing requires statistical baselines, pre-experiment estimation, post-experiment analysis, and the ability to run load tests alongside chaos injection.

## Decision

### Five-Phase Lifecycle

Every experiment follows five phases:

1. **Estimate** — Pre-experiment prediction: expected outcome, recovery time, degradation level, confidence. Compared against actuals in Phase 5 for accuracy tracking.

2. **Baseline** — Statistical characterization of normal behavior before fault injection. Configurable methods: mean ± sigma, percentile thresholds. Produces `BaselineResult` with derived tolerances.

3. **During** — Fault injection (the method) with concurrent steady-state probing. Background activities for load generation. Controls lifecycle hooks fire at each phase boundary.

4. **Post** — Recovery observation after fault injection. Measures time to recovery, residual degradation, and data integrity.

5. **Analysis** — Cross-run comparison: estimate accuracy, trend detection (improving/stable/degrading), resilience scoring (0-1), and regulatory evidence generation.

### Chaos Toolkit Compatibility

The model retains conceptual compatibility with Chaos Toolkit's experiment structure:

- **Steady-state hypothesis** → probes evaluated before and after the method
- **Method** → sequential actions with pause intervals
- **Rollbacks** → triggered on deviation (configurable: always, on-deviation, never)
- **Controls** → lifecycle hooks at experiment, activity, and hypothesis boundaries

### Load Integration

Load testing tools (k6, JMeter, Locust) run as **background activities** during the experiment. They start before baseline collection, run through fault injection, and stop after recovery observation. This provides load-contextualized resilience data.

### Baseline Methods

- **Mean + standard deviation** — `mean_stddev` with configurable sigma (default 2.0)
- **Percentile threshold** — `percentile` with configurable level (default p95)
- Warmup period excluded from statistical calculation
- Confidence level configurable (default 0.95)

## Consequences

- More complex experiment definition than simple hypothesis-method models
- All five phases are optional — a minimal experiment only needs a method
- Estimate accuracy improves with historical data (cross-run analysis in Phase 5)
- Background activities require async execution support (Phase 6)
