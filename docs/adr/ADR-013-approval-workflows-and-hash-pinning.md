---
title: "ADR-013: Approval Workflows and Hash Pinning"
parent: Architecture Decisions
nav_order: 13
---

# ADR-013: Approval Workflows and Hash Pinning (risk tiers, quorum, TTL, break-glass)

- Status: accepted
- Date: 2026-07-30

## Context

ADR-011 gave the daemon run control; ADR-012 gave it authenticated users
with roles. What was still missing is *change management*: any
authenticated operator could dispatch any registered definition against any
environment, immediately, with no human checkpoint. For a chaos platform
that is the gap every compliance framework names first (SOC 2 CC8.1,
DORA's change-management articles, ISO 27001 A.8.32): changes to
production-like systems must be authorized before they execute, by someone
other than the requester, against the exact content that was authorized.

The requirements that shaped the design:

- **Proportionality.** A probe-only dry validation cannot need the same
  ceremony as a multi-fault production experiment. Blanket approval queues
  teach operators to route around them.
- **What is approved is what runs.** Approving "run X" is meaningless if
  the definition or parameters can drift between approval and dispatch.
- **Approvals decay.** An approval granted Monday morning must not dispatch
  Friday night against a changed system.
- **A veto is a veto.** The autopilot policy engine (already in the stack)
  exists to say *no*; a human approval must never silently override that.
- **Emergencies exist.** There must be an escape hatch, and it must leave
  more audit evidence, not less.

## Decision

### Risk tiers classify every run at request time

`POST /api/runs` resolves and validates the definition (the same
`prepare_run` pipeline the CLI uses), introspects the resolved experiment,
and classifies it into a tier. The highest triggered rule wins:

| # | rule | tier |
|---|------|------|
| 1 | catalog hash match OR probe-only definition (no Action steps) | T0 |
| 2 | production-class environment (`prod`, `production`, `live`, …) | T3 |
| 3 | faults present AND no rollback declared | T3 |
| 4 | more than one distinct fault kind | T3 |
| 5 | staging-class environment (`staging`, `stage`, `stg`, `preprod`, `uat`, …) | T2 |
| 6 | destructive-named fault (`kill`, `delete`, `corrupt`, `wipe`, …) | T2 |
| 7 | otherwise (standard experiment) | T1 |

T0 runs enqueue directly (the T0 pre-approved catalog is not yet
configured, so today only probe-only definitions land here). T1–T3 park in
the new `pending_approval` run state. Classification inputs are frozen at
request time into the approval request; environment classification is
deliberately conservative (exact or separator-prefixed name matches —
`devprod` is *not* production, and unknown names fall to the T1/T2 rules,
never to T0).

### Quorum, segregation of duties, TTL, single-use

- **Quorum**: T1 and T2 need one approver, T3 needs two. Segregation of
  duties is enforced by the writer, not the UI: approver ≠ requester, one
  decision per approver (403/409 at the API).
- **TTL**: T1 72h, T2 24h, T3 4h. The values track the tier's blast
  radius and operational tempo: a standard dev experiment (T1) routinely
  waits a weekend for a second pair of eyes; a staging/destructive change
  (T2) is planned within a working day; a production change (T3) executes
  inside the change window it was approved for — four hours after approval
  the ambient facts the quorum agreed to are no longer trustworthy. A
  background sweeper flips lapsed requests to terminal `expired`; an
  expired request cannot be approved, dispatched, or break-glassed — it
  must be re-requested, re-classified, and re-approved.
- **Single-use**: one approval set dispatches one run, consumed at
  dispatch (`consumed_at_ns`). Re-running means re-requesting.

### Hash pinning: approve the inputs, re-verify at dispatch

Every approval request carries a pin: `SHA-256` over the canonical JSON of
`{definition_toon, params (BTreeMap), env, target}` — the *resolution
inputs*, not the resolved `Experiment` artifact. This is deliberate: the
resolved experiment contains `HashMap` fields whose serialization order is
nondeterministic, so hashing it cannot produce a stable pin, while the
inputs serialize deterministically (struct field order + sorted map). The
pin binds everything that determines the resolved artifact, because
resolution itself (`prepare_run`) is deterministic. The daemon's worker
re-resolves and re-verifies the pin at the last moment before execution;
any drift — edited definition, edited params, edited env/target — refuses
dispatch with a `dispatch_refused` audit event, even for a fully approved
run.

### T3 re-runs the autopilot gate at approval time

T3 approvals evaluate the tumult-autopilot policy engine in-process before
the decision is recorded: a synthetic candidate is built from the frozen
introspection (fault count, rollback, guard, steady-state — exactly the
fields the gate's validator reads) with `Trigger::Manual` and no earned
autonomy, against current ambient facts (runs in the last 24h, concurrent
active runs, business hours). The integration boundary is a library call
inside the daemon, configured by `KRONIKA_AUTOPILOT_POLICY`; the gate is
**fail-closed by construction** — no policy loaded, a non-Enact verdict,
or an evaluation error all refuse the approval (422). A **Veto can never
be overridden by an approval**; it is audited (`gate_veto`) and only
break-glass passes. The gate is not re-run at dispatch; the short T3 TTL
(4h) bounds how stale the verdict can get.

### Break-glass: the escape hatch leaves more evidence

`POST /api/runs/{id}/break-glass` (Admin only, mandatory justification of
≥10 characters, run must still be `pending_approval`) bypasses quorum and
TTL — never the pin — and dispatches. In exchange it creates compliance
debt: the request is stamped `break_glass` with actor and justification,
the override is audit-logged (`overridden`), and a retrospective
manual-evidence record is opened as an unscored `draft` naming the run,
the justification and the pin — it sits in the manual-evidence review
queue until a human completes the retrospective.

### Audit: hash-chained, actor-carrying

`run_audit` (schema v7) gains `prev_hash`/`new_hash`: every event's hash
chains onto the run's previous event, so a tampered or deleted row breaks
`verify_run_audit_chain`. Every approval-flow transition — `requested`,
`approved`, `rejected`, `gate_veto`, `overridden`, `dispatch_queued`,
`consumed`, `dispatch_refused` — records the real authenticated actor.
Reads ride fresh read-only connections; all mutations ride the daemon's
single-writer channel; the approval endpoints never open a write
connection of their own.

### Evidence: the R2 pack shows the chain

The evidence-pack report (SOC 2 mapping) gains an "Approval chain (change
management)" section — one row per approval-gated run in the period: tier,
env, requester, quorum progress, decisions, break-glass marker, consumed
marker, terminal state (CC8.1 change-management evidence).

## Consequences

- Every run creation now resolves the definition at request time: invalid
  definitions fail with 400 at request time instead of failing the run at
  dispatch (a behavior change from ADR-011's flow).
- `pending_approval` survives daemon restarts by design (it is not an
  active state — nothing is executing, nothing needs rollback); orphan
  reconciliation is untouched.
- Approval writes (decision + audit + state flip) ride the single-writer
  channel, so throughput of decisions is bounded by the writer loop —
  correct by construction, and far from a bottleneck at human decision
  rates.
- The T0 pre-approved catalog does not exist yet (`catalog_matched` is
  hardcoded `false`); wiring a catalog is future work and only lowers
  tiers, never raises them.
- The destructive-name heuristic (rule 6) is a heuristic — it catches
  obviously destructive faults by name and can be fooled by naming; the
  env-class and rollback rules are the structural guarantees.
- T3's ambient context is partially synthesized (`open_deviation_for_target`
  is `false`, `hours_since_last_run_on_service` unknown): a policy keyed on
  those facts evaluates conservatively, and fail-closed means a policy that
  *requires* them simply will not Enact.
- Demo environments without `KRONIKA_AUTOPILOT_POLICY` cannot approve T3
  runs at all — acceptable (T3 should be rare in a demo), but it must be
  documented for operators.
