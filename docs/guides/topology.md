---
title: Topology
parent: Guides
nav_order: 17
---

# Service topology, compliance lineage & injection recommendations

ChaosGraph can now answer the three questions that matter after a chaos run:
**where** in your service topology does compliance break, **why** (which
fault injection caused it), and **where to inject next** to improve your
compliance posture. This guide covers the declared-topology model, the
lineage semantics, the recommender, and the map renderings.

## Declaring a topology

Topology is *declared, never guessed* — services and their dependencies come
from a reviewed TOML file, so the map's shape is as trustworthy as the repo
that commits it:

```toml
# topology.toml
[[service]]
name = "gateway"
depends_on = ["api"]
tier = "edge"          # free-form; edge/service/data sort first on the map
owner = "team-platform"

[[service]]
name = "api"
depends_on = ["db"]
owner = "team-core"

[[service]]
name = "db"
tier = "data"
```

Rules:

- every `depends_on` target must be declared in the same document
- names are normalized exactly like run-derived hosts (scheme, path and
  `:port` stripped; **case preserved**) — declare names exactly as your
  experiments reference them (`upstream = "api:8080"` → `svc:api`), or the
  lineage cannot join runs to your map
- cycles are allowed; duplicates and unknown dependencies are rejected

Import (idempotent — re-import replaces the whole declared sub-graph):

```
tumult topology import topology.toml
```

Declared services get `depends_on` edges under the sentinel run id
`__topology__` and node attrs `declared/owner/tier`. Run ingestion merges
attrs, so re-running experiments never clobbers topology metadata.

## Compliance lineage

For every (compliance article, service) pair the lineage computes a status:

- **evidenced** — the latest run of every experiment declaring the mapping
  and targeting the service completed, producing an `evidences` edge
- **broken** — the latest run declared the mapping (`maps_to_compliance`)
  but produced **no** evidence, and the run exhibited a deviation. The cell
  carries the cause: the deviation, the tripped guard, failing actions, and
  — when attribution is unambiguous — the `caused_by` fault
- **untested** — no experiment connects the article to the service at all

Multi-experiment cells are **worst-of**: one currently-broken experiment
marks the pair broken even if another still evidences it (the audit-safe
reading). Causal attribution is conservative by design: a failed action
attributes to its fault; a guard halt with exactly one injected fault
attributes to that fault; anything ambiguous stays unattributed rather than
guessed.

```
tumult topology lineage --framework DORA            # text
tumult topology lineage --framework DORA --format json
```

## The map

```
tumult topology map --framework DORA                # text, with recommendations
tumult topology map --format mermaid                # paste into any mermaid renderer
tumult topology map --format json                   # structured, for tooling
```

Text shows one block per service (worst state first glyph), break causes
inline, and `>> RECOMMENDED` markers. Mermaid renders the dependency graph
with state-colored services (green evidenced, red broken, amber untested),
dashed cause annotations on broken services, and ⚡ recommendation nodes.

## Injection recommendations

```
tumult topology recommend --framework DORA --limit 3
```

Deterministic and explainable — every recommendation carries its reasons.
The score multiplies: compliance state (broken 1.0 / untested 0.6) ×
citation strength (direct/supporting/indirect) × topology criticality (how
many services depend on this one) × proximity to broken services × a bonus
for never-tested actions. No LLM in the loop; the same store always yields
the same ranking.

## MCP tools

The same four capabilities are exposed to agents:

| tool | role | purpose |
|---|---|---|
| `tumult_topology_import` | Operator | import/replace the declared topology |
| `tumult_topology_map` | Viewer | text/mermaid/json map with overlays |
| `tumult_compliance_lineage` | Viewer | per-(article, service) status + causes |
| `tumult_recommend_injection` | Viewer | ranked, explainable next injections |

Plus `tumult_chaosgraph_cypher` for arbitrary read-only openCypher queries
over the whole graph (see the ChaosGraph guide).

## Demo proofs

The single demo environment (`make demo`) carries the whole story:
`make demo-topology` runs three proof scenarios — green lineage, break with
attribution, and the full recommend→run→improve loop — capturing maps and
lineage JSON under `demo/proof/topology/`. See the blog post
[Where compliance breaks](../blog/24-topology-compliance-lineage.md) for the
rendered walk-through.
