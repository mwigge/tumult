---
title: "ADR-011: Daemon-Run Experiments"
parent: Architecture Decisions
nav_order: 11
---

# ADR-011: Daemon-Run Experiments (embedded runner, e-stop, orphan reconciliation)

- Status: accepted
- Date: 2026-07-30

## Context

Until now tumultd was a passive observer: the CLI executed experiments and
the daemon ingested their telemetry. That split has three costs:

- **No central run control.** Every executor is a short-lived CLI process;
  there is no place to list what is running *right now*, enforce admission
  control, or stop a run without `kill` and shell access to the executor
  host.
- **A killed executor leaves faults applied.** When the process running an
  experiment dies mid-method (OOM, `kill -9`, node loss), the rollback
  stack dies with it — the fault stays in the target system with no record
  of why.
- **Duplicated execution wiring.** Any second executor (daemon, scheduler)
  would have to re-implement the CLI's parse → resolve → validate → run
  pipeline.

The daemon is the natural home for run control: it already owns the store
(single-writer channel), serves the API the UI talks to, and is
long-lived. What it must never do is compromise the two properties the
platform is built on — the single-writer store model and bounded,
backpressured pipelines.

## Decision

### One execution path, shared

The CLI's provider executor moved into a new `tumult-exec` crate
(`ProviderExecutor` + the native plugin registry); CLI and daemon depend on
it. Validation is one function — `tumult_ingest::prepare_run` — applying
the exact CLI pipeline (parse, resolve config/secrets, template vars,
validate), so a definition the daemon accepts is definitionally one the CLI
would accept. Daemon-side runs reuse `tumult_core::runner::run_experiment`
unchanged: the runner's journal, rollback stack, guard/halt machinery and
e-stop `CancellationToken` all apply.

### Schema v4: registry, runs, audit

Three tables (added non-destructively, `CURRENT_SCHEMA_VERSION = 4`):

- `run_registry` — validated definitions, deduped by SHA-256 content hash;
  the registry id derives from the hash (`reg-<12 hex>`), so identical
  TOON always resolves to the same row and re-validation is idempotent.
- `runs` — one row per run: state machine (`queued → validating → running
  → stopping → passed|deviated|failed|aborted`, plus `orphaned` /
  `rollback_pending` for reconciliation), rollback status, params,
  timestamps, and the `experiment_id` link into the analytics family once
  the journal lands.
- `run_audit` — append-only event trail per run (`enqueued`, `started`,
  `stop_requested`, `orphan_detected`, `rollback_completed`, terminal
  state, …). Every mutation rides the daemon's single-writer channel; the
  run queue never opens a write connection of its own.

### Bounded queue, explicit backpressure

`RunQueue` executes on a fixed worker pool (`TUMULTD_RUN_CONCURRENCY`,
default 2) with the *waiting* queue bounded separately
(`TUMULTD_RUN_QUEUE_DEPTH`, default 32) by a semaphore permit held from
enqueue until a worker dequeues. Enqueue beyond capacity is rejected
before anything is persisted — the API maps it to 429. There is no
unbounded queue anywhere in the design, matching the ingest channel's
backpressure philosophy.

### E-stop is the runner's own token

`POST /api/runs/{id}/stop` cancels the run's `CancellationToken` — the
same e-stop primitive the CLI exposes — so the runner halts before the
next activity and its own rollback path unwinds the fault; the run ends
`aborted` with `rollback_status` recorded. A run still waiting is finished
as `aborted` before it starts (the worker re-checks state after dequeue).
Stopping an unknown run is 404, an already-terminal one 409.

### Startup orphan reconciliation

Before binding the servers, the daemon reads runs left in an active state
by a previous process lifetime and, per run: records `orphan_detected`,
attempts rollback via `run_orphan_rollback` (rollbacks only, strategy
`Always`), records the outcome, and finishes the run (`aborted` when
unwound or never started, `rollback_pending` when the rollback itself
failed — the one state that asks a human to look). A `kill -9` on the
daemon therefore no longer abandons an injected fault.

Known limitation: orphan rollback rebuilds the executor with an empty
injected environment — config/secret re-resolution is not attempted for a
process that no longer exists; providers that need injected env to roll
back may fail, and that failure is exactly what `rollback_pending`
surfaces.

### Telemetry loopback

The daemon initializes tumult-otel with its OTLP endpoint pointing at its
own gRPC ingest (localhost when bound to `0.0.0.0`), flushed by a drop
guard on every `serve()` exit path. Daemon-executed experiments emit the
same `resilience.*` spans/logs/metrics the CLI emits, so they appear in
the UI, reports, and scores indistinguishable from CLI runs — one
analytics surface for both origins.

## Consequences

- The API is no longer read-only in spirit: `/api/runs*` mutates, but only
  through the single-writer channel and fresh read-only readers — the
  store model is untouched.
- The daemon process now executes arbitrary experiment providers; its
  blast radius equals the CLI's. Operators should treat the daemon's
  environment (config/secrets resolution, network reach) with the same
  care as an executor host.
- Overload is visible (429) rather than queued; schedulers must retry.
- Runs executed while the daemon holds the store are reconciled on the
  next start, closing the crash gap — but reconciliation cannot re-resolve
  secrets (above), so `rollback_pending` needs an operator runbook.
