# Execution Flow

Tumult experiments follow a five-phase lifecycle. Each phase produces data that feeds into the next.

## Phase Overview

```
Phase 0: ESTIMATE     Record predictions before any measurement
Phase 1: BASELINE     Connect, sample, derive statistical thresholds
Phase 2: DURING       Inject fault, observe degradation continuously
Phase 3: POST         Remove fault, measure recovery
Phase 4: ANALYSIS     Compare estimate vs actual, compute scores
```

## Detailed Flow

### 1. Parse and Validate

```
tumult run experiment.toon
    │
    ├── Parse TOON → Experiment struct
    ├── Validate: method not empty, plugin refs exist
    ├── Resolve config (env vars, inline values)
    └── Resolve secrets (env vars, file paths)
```

### 2. Initialize

```
    ├── Init OTel (if enabled)
    ├── Start root span: tumult.experiment
    ├── Register controls (lifecycle hooks)
    └── Record estimate (Phase 0) in journal
```

### 3. Start Load (if configured)

```
    ├── Start k6/JMeter in background
    └── Wait for load to stabilize
```

### 4. Baseline (Phase 1)

```
    ├── CONTROL: before_hypothesis
    ├── Warmup: discard first N seconds
    ├── Sample: run probes at interval for duration
    ├── Derive: calculate mean, stddev, percentiles
    ├── Anomaly check: CV > 0.5? range > 10x median?
    ├── Derive tolerance bounds (method: mean±Nσ, percentile, IQR)
    └── CONTROL: after_hypothesis
```

### 5. Hypothesis BEFORE

```
    ├── Run all hypothesis probes
    ├── Evaluate tolerances
    └── If ANY probe fails → ABORT → skip to rollbacks
```

### 6. Method Execution (Phase 2)

```
    ├── CONTROL: before_method
    ├── For each step (sequential):
    │   ├── CONTROL: before_activity
    │   ├── Wait pause_before_s
    │   ├── Execute action/probe via plugin
    │   ├── Record OTel span + metrics
    │   ├── Wait pause_after_s
    │   ├── Record ActivityResult
    │   └── CONTROL: after_activity
    ├── Background activities run concurrently via tokio::spawn
    ├── During-phase sampling: continuous probes capture degradation curve
    └── CONTROL: after_method
```

### 7. Hypothesis AFTER

```
    ├── Run all hypothesis probes again
    ├── Compare against baseline-derived tolerances
    └── If ANY probe fails → mark DEVIATED
```

### 8. Recovery (Phase 3)

```
    ├── Sample probes at interval
    ├── Detect recovery point (all probes within tolerance)
    ├── Calculate MTTR
    ├── Check data integrity
    └── Record PostResult
```

### 9. Rollbacks

```
    ├── Evaluate rollback strategy:
    │   ├── always → execute
    │   ├── on-deviation → execute only if deviated
    │   └── never → skip
    ├── For each rollback action:
    │   ├── CONTROL: before_activity
    │   ├── Execute via plugin
    │   └── CONTROL: after_activity
    └── CONTROL: after_rollback
```

### 10. Stop Load

```
    ├── Stop k6/JMeter
    └── Collect load metrics (throughput, latency, error rate)
```

### 11. Analysis (Phase 4)

```
    ├── Compare estimate vs actual outcome
    ├── Calculate estimate accuracy
    ├── Determine trend (improving/stable/degrading)
    ├── Generate regulatory evidence
    └── Compute resilience score (optional)
```

### 12. Finalize

```
    ├── Determine ExperimentStatus:
    │   ├── completed (hypothesis met, all actions succeeded)
    │   ├── deviated (hypothesis failed after method)
    │   ├── aborted (hypothesis failed before method)
    │   ├── failed (action execution error)
    │   └── interrupted (Ctrl+C or timeout)
    ├── CONTROL: after_experiment
    ├── Write journal.toon
    ├── End root span
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
