# Autopilot: policy-gated autonomous fault injection

Autopilot closes the loop the topology/compliance work opened: triggers
(stale evidence, broken controls) feed the deterministic recommender, a
validator rejects experiments that cannot falsify anything, a 14-rule
safety gate decides enact / downgrade / propose / veto, and every decision
is persisted — with the full rule trace and the policy hash — *before*
anything runs.

## Quick start

```
tumult autopilot once --policy autopilot.toml                # dry: decide + record only
tumult autopilot once --policy autopilot.toml --execute      # enact what the gate allows
tumult autopilot status [--verdict propose] [--format json]  # queue + history
tumult autopilot approve <decision-id>                       # run a proposed playbook
tumult autopilot deny <decision-id> --reason "..."           # veto = ladder feedback
tumult autopilot export <dir>                                # parquet cold archive
```

```
tumult autopilot notify-change --service X --source deploy-webhook   # change-triggered revalidation
tumult topology discover-k8s [--namespace ns]...                     # proposed topology TOML (review, then import)
```

MCP: `tumult_autopilot_run` (Operator), `tumult_autopilot_status` (Viewer),
`tumult_autopilot_respond` (Operator), `tumult_autopilot_export` (Operator),
`tumult_autopilot_notify` (Operator).

Additional gate inputs: **dynamic guard-telemetry pre-flight** (the guard's
probe is executed once pre-enact; a failing probe downgrades — the gate will
not authorize a blast it cannot observe), **target enrollment**
(`require_enrollment` + `enrolled_services`: un-enrolled targets are vetoed
structurally), **change events** (recorded via `notify-change`; produce
`change_event`-triggered revalidation candidates for up to 7 days), and
**OTel-derived criticality** (`TUMULT_CRITICALITY_FILE` JSON of service →
observed span rate; demo one-liner:
`docker exec demo-signoz clickhouse client -q "SELECT serviceName, count() FROM signoz_traces.distributed_signoz_index_v3 WHERE timestamp > now() - INTERVAL 60 MINUTE GROUP BY serviceName FORMAT JSON"`).

## The policy file

```toml
[autopilot]
enabled = true                 # master switch; false vetoes everything
max_runs_per_day = 6           # hard daily enactment budget (veto)
cooldown_hours = 12            # per-service spacing (downgrade)
evidence_ttl_days = 90         # staleness trigger default
enact_tiers = ["service", "edge"]   # tiers where enact is ever allowed
require_guard = true           # no guard -> never auto-enact
business_hours_only = false
autonomy_threshold = 0.8       # clean-run ratio to graduate a fault class
autonomy_min_samples = 3

[autopilot.evidence_ttl_days_by_framework]
DORA = 90
NIS2 = 120

[[autopilot.pretrusted]]       # explicit operator consent, hashed with the policy
plugin = "tumult-containers"
action = "kill-container"
tier = "data"

[[autopilot.playbook]]         # curated experiment per fault class
plugin = "tumult-containers"
action = "kill-container"
service = "demo-postgres"      # optional narrowing
experiment = "demo/experiments/demo-topo-recommended.toon"
```

The sha256 of the policy text is stamped on every decision: a verdict is
reproducible from `(policy hash, recorded inputs)`, forever.

## Verdicts

- **enact** — all 14 rules pass: enabled, validator clean, playbook
  resolved, tier allowed, guard present and observable, no open deviation
  on the target, no concurrent experiment, budget and cooldown clear,
  confidence high, autonomy earned or pretrusted. With `--execute` the
  playbook runs (guards armed, rollback verified, post-run steady-state
  checked — a run that doesn't recover is `run_failed` and the class loses
  autonomy).
- **downgrade** — would have enacted, but one bounded condition failed
  (cooldown, business hours, tier, unverified guard telemetry, directional
  confidence, autonomy not yet earned). Lands in the human queue with the
  exact blocker named.
- **propose** — never enact-eligible (no playbook, validator blockers).
- **veto** — hard safety stop: disabled, hollow experiment, open deviation
  on the target, concurrent experiment, budget exhausted. Recorded, never
  run, names the rule that fired.

Approving runs the playbook; denying records feedback. Both are lifecycle
events on the immutable decision — nothing is ever rewritten.

## Earned autonomy

Fault classes (`plugin::action@tier`) start propose-only. A class
auto-enacts once its clean-run ratio over enacted runs crosses
`autonomy_threshold` with at least `autonomy_min_samples` — and drops back
on any veto, override, or failed recovery. `[[autopilot.pretrusted]]` is
the only shortcut, and it is deliberately loud in the policy file.

## Lineage and storage

Every decision is a `recommendation` node in ChaosGraph (verdict, article,
service, policy hash in attrs); enacted decisions link `rec:<id>
-[enacted]-> run:<experiment_id>` into the journal/evidence graph — one
query answers "why did this run?", including via `tumult_chaosgraph_cypher`.
Decisions and lifecycle events live in two insert-only DuckDB tables
(`autopilot_decisions`, `autopilot_events`; no update/delete surface
exists), written before the runner fires. `tumult autopilot export` copies
both to Parquet for the immutable long-horizon archive.

## Demo proof

`make demo-topology` runs the full proof suite against the one demo
environment — proof 4 is the autopilot story: pretrusted enact, ambient
vetoes protecting a broken service, cooldown downgrade on the repeat pass,
human denial recorded, graph lineage, parquet archive. The 2.15.0 release
gate was three consecutive clean 16-proof runs (2.16.0).
