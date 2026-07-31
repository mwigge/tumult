# Research: org hierarchy rollups (v0.5.0, Part A)

Status: implemented in v0.5.0 — see `tumult_compliance::org`, `GET /api/scores/tree`,
the UI's Scores page, and
[ADR-009](../adr/ADR-009-org-hierarchy-and-manual-evidence.md).

## Problem

v0.4.0 scores individual experiments and rolls them up by *target system*
and into a single portfolio number. Organisations do not think in target
systems; they think in **teams, units and domains**. A head of platform
asking "which part of my org is under-tested?" has no answer in Tumult.
Compliance frameworks push the same way: DORA Art. 24 expects a resilience
testing *programme* with coverage across the estate, and programme-level
reporting needs an organisational axis, not just a flat experiment list.

Requirements distilled from the brief:

- A single-parent hierarchy of arbitrary depth (team → unit → domain →
  company), defined declaratively, not in code.
- Experiments mapped onto teams by name pattern; nothing may silently
  vanish from the rollups.
- Some experiments matter more than others (a payments failover drill vs a
  batch-job soak): scores must be *weighted*.
- Rollups must reflect the estate's composition: a one-experiment team must
  not pull a domain's score as much as a ten-experiment team.
- Coverage next to every score: a 100 from one passing experiment out of
  twelve expected is not "good".

## Prior art

- **Backstage catalog** (`catalog-info.yaml`): the industry-default way to
  declare org structure in YAML — `Group` entities with `parent`/`children`
  and `spec.members`, loaded from a file and refreshed, never edited
  through the UI. We borrow the file-as-source-of-truth pattern and the
  single-parent group tree, simplified to one `nodes:` list.
- **Gremlin team scores**: per-team resilience scores as the mean of
  scenario outcomes, with staleness decay. We already follow this per
  experiment (v0.4.0); the question is how to *compose* team scores upward.
- **Drata/Vanta control rollups**: compliance dashboards aggregate control
  pass-rates per framework section, weighted by control criticality, and
  always show coverage (tested / total) next to the percentage. Their key
  UX lesson: an aggregate without a coverage denominator invites
  misreading.
- **DORA capability models** (the DevOps research programme, not the EU
  regulation): capabilities are assessed per team and rolled up with
  explicit weighting of "critical path" services — the same criticality
  idea as this feature's `critical|high|default` levels.

## The aggregation rule: weighted mean from leaves, never mean of means

The tempting implementation computes each team's score and averages the
child scores at each level. That is wrong whenever teams differ in size:

```
edge-team:   [100]                    mean 100
data-team:   [50, 50, 50]             mean 50
mean of means:        (100 + 50) / 2 = 75   ← one strong experiment
leaf-recomputed:      (100 + 50*3) / 4 = 62.5   cancels three weak ones
```

The mean-of-means flatters the domain by 12.5 points because the smallest
team happens to be the strongest. Tumult therefore recomputes every node
score from **all leaves in its subtree**:

```
S(n) = Σᵢ wᵢ·sᵢ / Σᵢ wᵢ      over all experiments i mapped into subtree(n)
```

with `wᵢ = defaults.weight × criticality(name)` and criticality levels
`critical = 3`, `high = 2`, default `1`. The weighted variant of the
example: if the edge experiment is `critical`, the domain score becomes
`(3·100 + 3·50) / 6 = 75` — the weight expresses importance, the leaf
count expresses composition; neither collapses the other.

This is enforced in `OrgTree::aggregate`, which walks the full leaf list
for every node rather than combining child results — O(leaves × nodes),
trivial at demo and real-world scale (hundreds of experiments, dozens of
nodes).

## Coverage

`coverage = scored / expected` per node, displayed as `scored/expected`:

- **expected**: every experiment mapped into the subtree — automated runs,
  verified manual records, *and* pending manual records (draft/submitted).
- **scored**: the subset contributing to the weighted mean (automated runs
  and verified manual records; inconclusive manual outcomes are excluded).

Pending records count toward `expected` so a team with five drafts and one
verified record reads as 1/6 covered rather than invisibly green. The
denominator can only count *known* experiments: glob assignments like
`checkout-*` cannot enumerate experiments that were never run, so
never-run-yet experiments are absent from both numbers (documented
limitation; the staleness decay handles known-but-old evidence).

## The (unassigned) bucket

Any experiment name matched by no assignment lands in a synthetic
`(unassigned)` node directly under the root, always visible in the treemap
and the tree table. Silently dropping unmapped experiments would let the
rollups look healthy while new experiments go unobserved; a visible bucket
turns "someone shipped an experiment without updating org.yaml" into an
obvious, drillable artefact. Assignment is first-match-wins in file order.

## Mapping syntax: `*`-only globs

Assignments match experiment names with glob patterns. Only `*` (any
character sequence, including empty) is supported — no `?`, no `[...]`
character classes, no `**` path semantics. This covers the actual naming
conventions seen in the wild (`checkout-*`, `*-failover`, `*cdn*`) while
keeping `glob_match` a twenty-line, fully unit-tested function instead of a
new dependency (`globset`). Node names must be unique across the file and
cannot contain `/`; parents reference names, URLs and the API use
slash-joined paths (`platform/edge/edge-team`).

## API and visualisation

`GET /api/scores/tree?node=<path>&range=…` returns one node: score, band,
period-over-period delta, coverage, a 10-point sparkline, and **one level**
of children (weakest first). One level per request keeps each call cheap
(the sparkline alone is ten as-of score computations) and maps naturally
onto click-to-drill navigation.

Visual choices considered for the root view:

- **Treemap (chosen)**: area ∝ Σ criticality weights of the subtree's
  leaves, hue = band (Okabe–Ito `#009E73` good / `#E69F00` fair /
  `#D55E00` poor — colour-blind safe, consistent with the v0.4.0 chart
  palette). Composition and health in one glance; click drills.
- Sunburst/icicle: prettier for deep trees but area is angle- or
  width-proportional to *counts*, harder to weight, and reads worse with 3
  levels.
- Indented tree table alone: precise but no composition overview; kept as
  the **non-root** view (score, band glyph, Δ, sparkline, coverage, weakest
  member, weakest-first).

Per-child Δ and sparklines in the tree table would multiply the cost (ten
as-of computations per child per render), so those two columns show the
current node's values and `—` for children — a deliberate, documented
trade-off.

## Limitations

- The tree is read at daemon start (`KRONIKA_ORG_FILE`, default
  `<db dir>/org.yaml`); edits need a restart. File-watch reload is
  deferred — the Backstage pattern shows operators version-control this
  file anyway.
- An experiment name present in *both* the automated and the manual world
  produces one scored leaf per origin; both map to the same team, so the
  team sees two leaves for one logical experiment. Documented; the fix
  (dedupe by name with origin precedence) is deferred until it bites.
- Pending manual records are read as of *now* for every sparkline sample
  point (the lifecycle's history lives in the audit trail, which scoring
  does not replay), so coverage history is approximate. Score history is
  exact.
- Node paths are validated server-side; unknown nodes and bad ranges
  return 400.
