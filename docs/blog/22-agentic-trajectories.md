---
title: "Agentic Trajectories: Chaos Engineering for Agents That Think in Steps"
parent: Blog
nav_order: 23
---

# Agentic Trajectories: Chaos Engineering for Agents That Think in Steps

*2026-07-04*

Here's a failure mode that never shows up in a single call. An agent retrieves some context at step 0. The retrieval is quietly poisoned — not empty, not an error, just wrong. Two steps later the agent writes an answer built on that context. The answer looks fine. It's valid JSON, it's confident, it even cites a doc. It's also ungrounded, and nothing between step 0 and step 2 flagged it.

If you fault-test that agent one call at a time, you'll never catch this. The retrieve call succeeded. The answer call succeeded. The bug lives in the *space between them* — in the trajectory.

2.6.0 ships multi-turn agent-graph fault modeling to test exactly that. You model an ordered sequence of model and tool calls, inject a fault at a specific step, and let the consequences propagate forward the way they do in a real agent. Then you assert over the *whole* sequence: did it recover, did it loop, did it terminate healthy, did it stay in budget.

This is the thing the rest of the chaos market doesn't do. Fault injection for agents, where it exists at all, injects into a single call — a slow model, a timed-out tool, a garbled response. That's useful, and Tumult does it too. But agents don't fail one call at a time. They fail across graphs.

## A trajectory is a graph, and faults cascade through it

The per-call engine answers "what does one model or tool call do under a fault?" A trajectory answers the harder question: "what does an ordered sequence of calls do when you break one step?"

```
   fault injected here
   (retrieval_poisoning)
          │
          ▼
   ┌──────────┐     ┌──────────┐     ┌───────────────┐
   │ step 0   │     │ step 1   │     │ step 2        │
   │ retrieve │────►│ plan     │────►│ answer        │
   │ (tool)   │     │ (model)  │     │ (model)       │
   └──────────┘     └──────────┘     └───────────────┘
        │                                  ▲
        │  poisoned docs carried forward   │
        └──────────────────────────────────┘
                                     consumes_retrieval = true
                                     → answer is ungrounded
                                     → terminates_healthy FAILS
```

The key move is that retrieval context *propagates forward*. Documents retrieved (or poisoned) at an early step flow into any later step that consumes retrieval. So a grounding failure doesn't stay local to the call where you injected it — it cascades downstream and surfaces as a broken answer two turns later, exactly like a real RAG agent. The fault is at step 0; the contract that fails is at step 2.

That's the whole reason single-call testing misses this class of bug. The damage and the symptom are in different places.

## Contracts that only make sense across turns

Per-step contracts (valid JSON, required citation, retry budget, fallback used) still run on each step. On top of them sit **whole-trajectory contracts** — assertions you simply can't express against one call:

- **recovers-within** — after the first unhealthy step, a healthy step must follow within N turns. Does the agent bounce back, or stay broken?
- **no-repeated-step** — no two steps may share the same signature. A repeat is a loop / infinite-reflection signal.
- **terminates-healthy** — the final step must be healthy. Did the agent end in a good state?
- **step-budget** — the trajectory must not exceed N steps. Did it run away?

## Three packs, three honest outcomes

2.6.0 bundles three trajectory packs. They're not all green-lights — the point is to show the machinery catching real agent-graph failure modes *and* correctly passing a resilient one.

```
  PACK                    FAULT              →  HEADLINE CONTRACT     OUTCOME
  ─────────────────────   ────────────────      ──────────────────   ───────
  rag-grounding-failure   poison @ step 0    →  terminates_healthy   FAIL
                          cascades to step 2    (final_step_unhealthy)

  reflection-loop         retry pressure     →  no_repeated_step     FAIL
                          @ step 0              (loop_detected)

  multi-tool-cascade      tool failure       →  recovers_within      PASS
                          @ step 1, contained   (fallback @ step 2)
                          by fallback
```

The RAG pack poisons retrieval at step 0 and watches the answer at step 2 lose its grounding — `terminates_healthy` fails with `final_step_unhealthy`. The reflection pack applies retry pressure that sends the agent into an identical-step loop — `no_repeated_step` catches it with `loop_detected`. And the cascade pack fails a tool at step 1, but the synthesize step at index 2 falls back to a cached result and terminates healthy — so `recovers_within` correctly *passes*. A resilient agent should pass, and this one does.

## Four subscores, one number

Where a single call gets scored on its operational envelope (latency, retries, cost, recovery), a trajectory gets scored on four agent-shaped dimensions. Each isolates the contracts that speak to it, then they roll into one severity-weighted resilience score:

```
  per-step contracts ─────► correctness_under_fault ┐
                                                     │
  recovers_within  ┐                                 │
  terminates_...   ┴──────► recovery ────────────────┤
                                                     ├──► resilience_score
  step_budget ────────────► cost_control ────────────┤    (severity-weighted
                                                     │     pass rate over all)
  no_repeated_step ───────► loop_avoidance ──────────┘
```

You get the headline number *and* the breakdown that explains it. A 0.556 with recovery pinned at 0.000 tells you the agent didn't fail randomly — it failed to recover.

## The real thing, verbatim

This is authentic output from `tumult agentic trajectory --pack rag-grounding-failure` — no editing:

```
injected: retrieval_poisoning @ step 0
step[2] answer (model) fault=none healthy=false
trajectory_contract: terminates_healthy = fail (final_step_unhealthy)
trajectory_contract: no_repeated_step = pass (ok)
trajectory_contract: step_budget = pass (ok)
headline_contract: terminates_healthy
expected: trajectory_contract_failed:final_step_unhealthy
actual: trajectory_contract_failed:final_step_unhealthy
subscore.recovery: 0.000  subscore.loop_avoidance: 1.000
subscore.correctness_under_fault: 0.600  resilience_score: 0.556
```

Read it top to bottom and the cascade is right there. The fault went in at step 0. Step 2 — the answer — comes out unhealthy even though *no fault was injected at step 2* (`fault=none`). The poison rode the retrieval context forward. `terminates_healthy` fails, `no_repeated_step` and `step_budget` pass, and the subscores explain why the overall landed at 0.556: recovery cratered, loop avoidance held.

## Where this sits in Tumult

The packs run against bundled deterministic fake adapters. No network, no API keys, no provider account — the whole scenario executes in-process against metadata baselines.

```
   ┌──────────────────────────────────────────────────┐
   │  tumult agentic trajectory --pack <name>          │
   │                                                   │
   │   ┌─────────────────────────────────────────┐     │
   │   │ execute_trajectory()                     │     │
   │   │   steps → inject fault → carry context   │     │
   │   │   → per-step contracts                   │     │
   │   │   → trajectory contracts → subscores     │     │
   │   └─────────────────────────────────────────┘     │
   │                    │                               │
   │            fake trajectory adapters                │
   │            (deterministic, seeded)                 │
   │                    │                               │
   │            no network · no API keys                │
   └──────────────────────────────────────────────────┘
              │
              ▼
   runs in the standard demo sweep
   (demo-agentic-trajectory), MCP list-scenarios
   surfaces the pack metadata
```

That deterministic-by-design choice is deliberate, and worth being honest about: these packs model agent-graph failure modes against fake adapters so the same fault produces the same trajectory every time — reproducible, seedable, CI-friendly. They are not pointed at your live agent. When you want to fault-test a real agent CLI, that's the `tumult-agent-cli` path, which already shipped — the adapter layer that drives Claude Code and Codex non-interactively. Trajectory packs are the *model* of how an agent graph breaks; the live-client path is where you point it at the real thing.

## Try it

```bash
tumult agentic trajectory --pack rag-grounding-failure
tumult agentic trajectory --pack reflection-loop
tumult agentic trajectory --pack multi-tool-cascade
```

Three packs, three failure modes, no setup. Watch a poisoned retrieval at step 0 quietly break an answer at step 2 — and watch Tumult catch it across the trajectory, where single-call testing can't look.

---

*Multi-turn agent-graph fault modeling ships in 2.6.0 in the `tumult-agentic` crate: `execute_trajectory` with per-step and whole-trajectory contracts, four agentic subscores, and three bundled packs. Validated on the live demo (`demo-agentic-trajectory`) in the standard sweep. The [agentic scenarios guide](../guides/agentic-scenarios.md) covers the pack model; [Bring Your Own Agent](18-agentic-recommendations.md) and the live-client path cover pointing it at a real agent CLI.*
