---
title: Execution Flow
parent: Guides
nav_order: 2
---

# Execution Flow

Tumult experiments follow a five-phase lifecycle. Each phase produces data that feeds into the next. The execution is orchestrated by the `run_experiment()` function in `tumult-core::runner`.

## Phase Overview

```
Phase 0: ESTIMATE     Record predictions before any measurement
Phase 1: BASELINE     Connect, sample, derive statistical thresholds
Phase 2: DURING       Inject fault, observe degradation continuously
Phase 3: POST         Remove fault, measure recovery
Phase 4: ANALYSIS     Compare estimate vs actual, compute scores
```

## Implementation

The runner takes four inputs:
- `Experiment` — the parsed experiment definition
- `ActivityExecutor` — trait for executing actions/probes (plugin system)
- `ControlRegistry` — lifecycle event handlers
- `RunConfig` — rollback strategy, baseline mode, dry-run flag

Returns a `Journal` containing the complete experiment results with all phases.

## Detailed Flow

### 1. Parse and Validate

```
tumult run experiment.toon
    │
    ├── Parse TOON → Experiment struct   (engine::parse_experiment)
    ├── Validate: method not empty       (engine::validate_experiment)
    ├── Resolve config (env vars, inline) (engine::resolve_config)
    └── Resolve secrets (env vars, files) (engine::resolve_secrets)
```

### 2. Initialize

```
    ├── CONTROL: BeforeExperiment
    ├── Generate experiment UUID
    ├── Record start timestamp (epoch nanoseconds)
    └── Record estimate (Phase 0) — preserved in journal as-is
```

### 3. Start Load (if configured)

```
    ├── Start k6/JMeter in background
    └── Wait for load to stabilize
```

### 4. Baseline Acquisition (Phase 1)

Baseline acquisition uses `tumult-baseline::acquisition::derive_baseline()`:

```
    ├── Warmup: discard first N seconds of samples
    ├── Sample: collect probe values at interval for duration
    ├── Per-probe statistics: mean, stddev, p50, p95, p99, min, max
    ├── Anomaly check: CV > 0.5? range > 10x median? insufficient samples?
    ├── Derive tolerance bounds (method: mean±Nσ, percentile, IQR)
    └── Produce AcquisitionResult with ProbeStats per probe
```

### 5. Hypothesis BEFORE

```
    ├── CONTROL: BeforeHypothesis
    ├── For each probe in hypothesis:
    │   ├── CONTROL: BeforeActivity{name}
    │   ├── Execute probe via ActivityExecutor
    │   ├── Evaluate tolerance (exact, range, or regex)
    │   └── CONTROL: AfterActivity{name}
    ├── If ANY probe fails tolerance → HypothesisResult.met = false
    ├── CONTROL: AfterHypothesis
    └── If not met → ABORT → skip method, go to rollbacks
```

### 6. Method Execution (Phase 2)

```
    ├── CONTROL: BeforeMethod
    ├── For each activity in method:
    │   ├── CONTROL: BeforeActivity{name}
    │   ├── Execute via ActivityExecutor
    │   ├── Build ActivityResult with status, output, timing, trace IDs
    │   └── CONTROL: AfterActivity{name}
    └── CONTROL: AfterMethod
```

### 7. Hypothesis AFTER

```
    ├── CONTROL: BeforeHypothesis
    ├── Run all hypothesis probes again (same as step 5)
    ├── CONTROL: AfterHypothesis
    └── If ANY probe fails → status = DEVIATED
```

### 8. Recovery (Phase 3)

```
    ├── Sample probes at interval
    ├── Detect recovery point (all probes within tolerance)
    ├── Calculate MTTR
    ├── Check data integrity
    └── Record PostResult
```

### 9. Determine Status

```
    ├── engine::determine_status(hypothesis_before, hypothesis_after, actions_succeeded)
    │   ├── hypothesis_before failed → Aborted
    │   ├── actions failed → Failed
    │   ├── hypothesis_after failed → Deviated
    │   └── all passed → Completed
```

### 10. Rollbacks

```
    ├── execution::should_rollback(strategy, deviated):
    │   ├── Always → execute
    │   ├── OnDeviation → execute only if deviated or aborted
    │   └── Never → skip
    ├── CONTROL: BeforeRollback
    ├── For each rollback action:
    │   ├── CONTROL: BeforeActivity{name}
    │   ├── Execute via ActivityExecutor
    │   └── CONTROL: AfterActivity{name}
    └── CONTROL: AfterRollback
```

### 11. Stop Load

```
    ├── Stop k6/JMeter
    └── Collect load metrics (throughput, latency, error rate)
```

### 12. Analysis (Phase 4)

```
    ├── runner::compute_analysis():
    │   ├── Compare estimate.expected_outcome vs actual ExperimentStatus
    │   ├── estimate_accuracy: 1.0 if prediction matched, 0.0 otherwise
    │   ├── resilience_score: 1.0 if completed, 0.0 otherwise
    │   └── estimate_recovery_delta_s: (actual - estimated) recovery time
```

### 13. Finalize

```
    ├── Record end timestamp
    ├── Calculate duration_ms
    ├── CONTROL: AfterExperiment
    ├── Build Journal with all phases and results
    ├── Write journal.toon
    └── Flush OTel
```

## Controls

Controls receive lifecycle events at each stage. They are registered at experiment start and called synchronously at each hook point. Common uses:

| Control | Purpose |
|---------|---------|
| Logging | Structured log output at each phase |
| Tracing | Additional span attributes or annotations |
| Safeguards | Abort if safety conditions are breached |
| Notifications | Send alerts on deviation or failure |

## Rollback Strategy

| Strategy | Behavior |
|----------|----------|
| `always` | Execute rollbacks after every experiment regardless of outcome |
| `on-deviation` | Execute rollbacks only when steady-state hypothesis fails (default) |
| `never` | Never execute rollbacks — use when the experiment is naturally idempotent |

## Error Handling

- Plugin execution errors are captured in ActivityResult, not propagated — the experiment continues
- SSH connection failures trigger retry with exponential backoff
- OTel export failures are logged but never block execution
- Ctrl+C triggers graceful shutdown: stop method, run rollbacks, flush OTel, write journal
