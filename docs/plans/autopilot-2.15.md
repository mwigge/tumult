# Autopilot — tumult 2.15 plan

*2026-07-07 · planning document · informed by market research (agentic-QE/AI-SRE
and chaos-automation landscape, mid-2026) and by the decision-pipeline
architecture proven in the sluss PR-gate project.*

## Positioning: the empty lane

The market bifurcates cleanly. AI-SRE agents (Datadog Bits, Resolve, Cleric,
Traversal, both OpenSREs) are *reactive* — they diagnose incidents and never
inject. Proactive chaos platforms either recommend without enacting (Gremlin
Reliability Intelligence, Steadybit Advice, Harness AI Reliability Agent) or
run autonomously only pre-production with no gating concept (Antithesis).
Compliance offerings (Gremlin for DORA, AWS FIS DORA guidance) are templates
and after-the-fact evidence reports.

**Nobody selects the next experiment from a compliance obligation.** Nobody
records recommendation → gate verdict → run → outcome → evidence as a
queryable lineage. Nobody treats human vetoes as first-class recommender
feedback. Nobody's gate emits *downgrade* — the industry is binary
allow/deny. All four are exactly what ChaosGraph + the 2.14 lineage work give
us nearly for free. DORA (in force, supervisory testing expectations rising)
is the demand driver to name.

**The headline property:** every autopilot decision is reproducible from
`(policy version, inputs)` — a deterministic gate over a deterministic
recommender, with the whole decision chain in the graph. Harness gestures at
this (AI proposes, ChaosGuard restricts) but guarantees neither determinism
nor lineage.

## Architecture: propose → gate → enact, all audited

```
trigger ──> recommender ──> validator ──> safety gate ──> runner ──> journal
  │             │               │             │              │          │
  └──── every arrow appends to the decision store BEFORE the next fires ┘
```

- **Triggers** (event-driven, not just cron): evidence TTL expiry per
  (control, service); control flips to broken; topology import adds an
  untested service; deploy/change events (change-triggered invalidation, per
  Azure mission-critical guidance); manual "revalidate now".
- **Recommender**: the existing deterministic engine, extended (below).
- **Validator** (pre-gate, from agentic-qe's anti-hollow-test idea): reject
  experiments that cannot falsify anything — no steady-state probe, no
  observable metric, no rollback, unbounded blast radius. Hollow experiments
  never reach the gate.
- **Safety gate**: deterministic policy evaluation, deny by default,
  verdicts: `enact` / `downgrade` / `propose` (human approval queue) /
  `veto` (with the rule that fired). *Downgrade* — auto-shrinking scope,
  duration, or target tier to fit policy — is novel in the market.
- **Audit-before-act**: decision row + graph delta written and flushed
  before the runner fires. A crashed autopilot must leave "decided, gate
  said yes, policy v3" even if no journal ever arrives.

## The gate policy (`~/.tumult/autopilot.toml`)

Speak ChaosGuard's canonical vocabulary — subject × fault-type × target ×
time-window — plus the six patterns the industry converges on and three it
doesn't have:

Industry-canonical (all six, table stakes):
1. steady-state assertion required on the spec, evaluated during the run,
   auto-halt + rollback (tumult guards — already exist)
2. structural blast-radius ceiling (target allowlist per tier/environment;
   Azure-style: un-enrolled target = injection impossible)
3. time windows + deploy-freeze awareness
4. policy-before-run, deny by default
5. universal emergency stop (CLI + MCP + control panel)
6. ambient-health precondition: never inject into an already-degraded system
   (check open deviations + steady-state of the *target's dependents*)

Tumult-novel:
7. **global impact ledger** (from ChAP's 5%-budget lesson): concurrent
   experiments budgeted *together*; autopilot holds one fault at a time in
   v2.15
8. **gate-telemetry pre-flight** (from the FIS/Azure misconfigured-alarm
   failure mode): before enacting, verify the guard's probe can actually
   observe the blast — "can I see what I'm about to break?"
9. **earned autonomy per fault class** (from Cleric): each (plugin, action,
   tier) class starts at `propose`; graduates to `enact` only when its
   precision score (enacted runs that completed without veto/override,
   window-decayed) crosses the policy threshold. Human vetoes reset the
   ladder. Autonomy is earned from evidence, not configured from hope.

## Recommender upgrades

- **Evidence TTL + decay** (Gremlin's is the industry's clearest: 1-week
  freshness, expiry states): per-article TTL in policy (DORA quarterly,
  etc.); staleness becomes a scoring factor and a trigger. "Which resilience
  claim expires soonest" is the compliance-driven selection nobody does.
- **Criticality scoring** (Monocle): weight by dependents-in-degree (have),
  plus traffic/RPS when OTel-derived signals land (later).
- **Coverage-guided steering** (Antithesis): bias toward unexplored
  (service × fault-class) cells; deprioritize recently-run (inverse
  staleness — Monocle did this).
- **Confidence tiers** (Traversal): recommendations carry a tier; gate keys
  on it (high → eligible for enact; directional → propose-only).

## Decision store: Arrow + DuckDB + Parquet

Sluss's SQLite was right for high-frequency small appends; tumult's decision
data is the opposite shape — low volume, long retention, analytically joined
against journals/graph/citations. So: extend the existing spine.

- `autopilot_decisions` table via the existing Arrow ingest path:
  recommendation snapshot (service, action, article, score, per-factor
  reasons), validator result, gate verdict + fired rule, policy version +
  policy content hash, trigger, confidence tier, autonomy-class score at
  decision time, timestamps, journal id (nullable until run completes).
- Graph lineage: `Recommendation` node kind; edges
  `gate_enacted`/`gate_vetoed`/`gate_downgraded`/`gate_proposed` → run;
  human responses (`approved`/`vetoed` + reason) as events. Queryable via
  lineage tools and `tumult_chaosgraph_cypher`.
- **Append-only by API** (DuckDB has no triggers): insert-only surface, no
  update/delete methods exist.
- **Cold archive: quarterly Parquet exports** (`COPY … TO parquet`),
  hash-chained manifest for tamper evidence. DuckDB file = working set;
  Parquet = the audit-grade record any tool can read in 2036.
- **Evidence-linking invariant** (Tracer OpenSRE): every decision references
  the raw signal ids (lineage cell, staleness computation, health probe
  results) that justified it — not summaries.

## The offline scoring harness ("test the autopilot before it tests you")

From Tracer OpenSRE's simulation harness: a replay corpus of historical
decisions, vetoes, and adversarial scenarios (already-degraded target,
misconfigured guard, compound blast radius). Any change to policy or
recommender scoring runs the corpus and reports verdict flips — gate
regression tests, run in CI. The 2.14 demo proofs seed the corpus.

## War-story-derived hard rules

1. Bound *worst-case* impact, not expected (ChAP's circuit breaker fired
   after real impact — detection latency is residual risk): v2.15 autopilot
   enacts only guard-halting, single-fault, rollback-verified experiments.
2. Rollback of the injection ≠ recovery of the system (Google DiRT): the
   post-run steady-state re-check must pass before the decision closes;
   failure opens a deviation and freezes the fault class to `propose`.
3. High-risk classes (data tier, prod) never graduate past `propose` in
   v2.15 — the DiRT "VP approval" tier, encoded.

## Explicitly not doing

- LLM as the decider (keep it authoring-only, behind the same gate).
- Agent-swarm orchestration (agentic-qe's 60-agent fleet) — coordination
  cost without decision-quality gain here.
- A single blended "reliability score" (Gremlin) — it compresses away the
  lineage that is the whole point.
- Auto-remediation. Autopilot injects and verifies; it does not fix.

## Phasing

**2.15.0** — decision pipeline end to end, conservative defaults:
`tumult autopilot` daemon (also `--once` for cron/CI); triggers: staleness +
broken-control + manual; validator; gate with verdicts + policy TOML;
decision store (DuckDB tables + graph lineage + Parquet export command);
earned-autonomy ladder (data collection from day one, `enact` unlocked only
for local/container tier); approval queue surfaced via MCP tool + control
panel card; `tumult autopilot log/status`; offline harness v1 (replay +
verdict-flip report); demo: autopilot runs against the one demo env with
proofs (staleness trigger fires → gate downgrades a prod-tier proposal →
human approves → run → evidence flip, all asserted from the store).

**2.15.x / later** — OTel-derived criticality signals; change-event
triggers (deploy webhooks); coverage-guided steering refinements;
Kayenta-style differential analysis where traffic supports it; k8s target
enrollment discovery.

## Success criteria

- Every autopilot action answerable by one query: *why did this run?* —
  trigger, scores, reasons, policy version, gate rule, human involvement.
- Gate verdicts bit-reproducible from (policy hash, decision inputs).
- Zero enactments outside policy in the replay corpus, ever.
- The demo proof: a compliance gap detected, gated, approved, closed, and
  archived to Parquet — without a human choosing the experiment.
