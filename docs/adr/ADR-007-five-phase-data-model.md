# ADR-007: Five-Phase Data Model

**Status:** Accepted
**Date:** 2026-03-29
**Decision Makers:** mwigge

## Context

Traditional chaos engineering tools follow a two-phase model: check steady state before fault injection, inject the fault, check steady state after. This misses the degradation curve during fault injection (how the system degrades, not just whether it recovered), provides no mechanism for prediction tracking (did the team expect the right outcome?), and produces insufficient evidence for regulatory compliance frameworks like DORA, NIS2, and PCI-DSS that require documented resilience validation with measurable baselines.

## Decision

Adopt a five-phase data model for experiment execution:

1. **Estimate** -- Team records predictions before execution: expected impact, expected recovery time, expected degradation pattern. Tracked for calibration over time.
2. **Baseline** -- Statistical measurement of steady-state behavior before fault injection. Can be static (fixed thresholds), statistical (mean plus/minus N standard deviations from sampled data), or learned (adaptive query execution). Produces quantified baselines, not just pass/fail checks.
3. **During** -- Continuous probe sampling during fault injection captures the full degradation curve: how fast the system degrades, whether it oscillates, and at what level it stabilizes.
4. **Post** -- Recovery measurement after fault removal: time to recovery, recovery completeness, whether the system returns to baseline or settles at a new steady state.
5. **Analysis** -- Cross-run learning: compare this execution against historical runs, update prediction calibration scores, generate regulatory evidence artifacts, feed resilience scoring models.

## Consequences

### Positive
- Scientific rigor: captures the full degradation and recovery curve, not just before/after snapshots
- Prediction tracking enables team learning and calibration measurement over time
- Regulatory evidence chain: each phase produces auditable artifacts suitable for DORA, NIS2, and PCI-DSS compliance
- Baselined thresholds are self-calibrating -- statistical baselines adapt to system evolution without manual threshold updates
- During-phase data enables degradation pattern classification (graceful, cliff, oscillating, cascading)

### Negative
- More complex execution flow compared to simple before/after models
- Longer experiment duration due to baseline sampling phase (statistical baseline requires multiple samples)
- More data to store and process per experiment run
- Higher barrier to entry for simple experiments that do not need all five phases

### Risks
- Baseline phase duration must be configurable; overly long baselines will discourage adoption for quick validation runs
- Analysis phase depends on historical data availability; first runs produce limited cross-run insights
- The five-phase model may be over-engineered for simple smoke-test experiments; the engine should support graceful phase skipping (e.g., skip Estimate for automated runs)
