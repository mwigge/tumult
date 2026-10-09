---
title: Execution Flow
parent: Guides
nav_order: 2
---

# Execution Flow

The experiment runner executes declared probes, faults and cleanup, and returns
a journal. The five-phase model names the intended evidence categories;
statistical baseline acquisition and several analysis fields are not integrated
into ordinary execution. [Data Lifecycle](../data-lifecycle.md) identifies
implemented and reserved measurements.

## Runtime inputs

`run_experiment()` receives an `Experiment`, an `ActivityExecutor`, a
`ControlRegistry` and `RunConfig`. The executor selects process, script or
native providers. CLI, MCP and daemon use the shared provider implementation;
target access, credentials and installed tools still determine which actions
can run. [Production Deployment](production-deployment.md#provider-capabilities)
lists those prerequisites.

`run_experiment_with_sampling()` additionally takes `SamplingConfig`:

| Field | Default | Meaning |
|---|---|---|
| `interval` | 1s | Pause between during/post sampling rounds |
| `max_during_samples` | 300 | Maximum during-phase rounds |
| `recovery_timeout` | 30s | Post-phase sampling timeout |

Experiments without hypothesis probes skip sampling. A probe must execute
successfully and satisfy its tolerance to pass.

## Normal execution order

1. The entrypoint parses, resolves configuration/secrets and validates the
   definition. Daemon runs also require an authorized execution binding and
   any required approvals before dispatch; see [Execution bindings](execution-bindings.md).
2. The runner records the optional estimate and invokes `BeforeExperiment`.
   Automatic statistical baseline acquisition does not run here.
3. It evaluates the steady-state hypothesis. A failed precondition aborts the
   method and applies the selected rollback strategy.
4. It starts configured load after the precondition succeeds. Missing load
   executor support is rejected before activities; load startup failure prevents
   method execution. Install the load tool and supply its script before use.
5. It starts during-phase probe sampling and guard monitoring, then executes
   the method. Background actions honor the concurrency limit. Cancellation
   interrupts pre-action pauses and waiting slots and prevents later dispatch.
6. It stops the during sampler and guard monitor. A guard breach or cancellation
   stops the method, stops load, and applies the rollback strategy. Such a run
   cannot be reported Completed.
7. On the normal path, it samples post-phase probes immediately after method
   completion, before cleanup. Sampling ends on the first passing round,
   cancellation or timeout. There is no automatic data-integrity verification
   or requirement for sustained recovery.
8. It stops load, collects its result, and evaluates the final hypothesis
   unless cancellation has been requested.
9. It determines status and applies the rollback strategy. A rollback is an
   explicitly supplied activity; the runner does not infer how to undo a fault.
10. It computes available analysis fields, records rollback failures, invokes
    `AfterExperiment`, and returns the journal. The entrypoint persists evidence
    and flushes configured telemetry.

A running provider call may finish or reach its own timeout before a stop can
complete. A stop request is not proof of instantaneous target cleanup. Inspect
the terminal status and every rollback result before declaring recovery.

## Baseline-only observation

`--baseline-mode only` takes a separate path: it evaluates the declared
hypothesis probes once and returns an observation journal. It excludes
controls, method, load and rollback, and the CLI skips automatic fault-evidence
ingestion. `full` and `skip` currently both use declared tolerances; neither
silently derives a statistical baseline.

## Rollback strategy

| Strategy | Behavior |
|---|---|
| `always` | Run supplied rollbacks after normal execution and on abort/interruption paths that require cleanup. |
| `on-deviation` (default) | Roll back on failed preconditions, deviation, guard halt, or failure/interruption after a fault starts. |
| `never` | Skip rollback, including interruption; the operator is responsible for cleanup. |

Baseline-only observation never runs rollback. Cancellation before a method
starts does not initiate cleanup for a fault that was never dispatched.
Rollback failures are recorded separately; a terminal run status alone is
insufficient evidence of a clean target.

## Controls and errors

Controls receive synchronous lifecycle events and can execute code; treat them
as part of the approved experiment. Ordinary provider failures are recorded as
activity results. Failed steady-state probes prevent injection; a matching
output string cannot override a failed process exit. Definition/dispatch/load
setup errors fail the run rather than silently omitting configured work.

Daemon dispatch persists its start and approval state before injection.
Restart reconciliation retries unresolved cleanup using the persisted recovery
plan. Recovery can still fail if credentials, target reachability or provider
tools are unavailable; examine recovery audit events and failed rollbacks.
