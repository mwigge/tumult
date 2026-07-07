---
title: ChaosGraph
parent: Guides
nav_order: 13
---

# ChaosGraph

ChaosGraph is a typed knowledge graph over Tumult's chaos data. It is built from experiment journals **as they ingest** and served to AI agents over MCP as compact sub-graphs, so an agent can answer "what did this experiment touch?" without re-reading a whole journal.

It shipped in **2.4.0** and is validated end to end on the live demo.

## Why a graph?

Agents burn context on the wrong thing. When an assistant wants to know which fault an experiment injects and which service it lands on, it does not need the steady-state hypothesis, the per-activity method results, the trace/span ids, or the analysis block. It needs a handful of relationships.

The win is token cost, and it compounds with history. A **targeted** `tumult_chaosgraph_neighbors` call (with a `rel` filter) answers "what fault does this experiment inject, what service does it hit?" in about **110 tokens**, and that answer stays the same size however many times the experiment runs. The raw journal is about **480 tokens** — and the corpus grows by another journal every run. So the graph is roughly **8× more compact per run**, answers store-wide questions in about **20× less** than reading all journals, and keeps a targeted answer **bounded (O(1))** where journal reading is **O(runs)**. Every number here is reproducible against the live demo with `make demo-proof`.

```
per run of history:   journals ──► +~480 tokens each   (unbounded)

                      chaosgraph ─► +~one small node    (targeted answer stays ~110 tokens)
```

The graph collapses each run to a small set of `(src)-[rel]->(dst)` tuples, so the agent reads relationships instead of records.

## The model

ChaosGraph is intentionally small: eight node kinds, ten edge relations — including the declared service topology (`depends_on`) and deviation cause attribution (`caused_by`) that power the [compliance lineage map](topology.md).

```
                 injects
   ┌────────────┐──────────►┌────────────────────────┐
   │ experiment │           │ fault                  │
   │            │           │ (plugin::function)     │
   └────────────┘           └────────────────────────┘
     │       │                       │
     │       │ targets               │ observed_on
     │       ▼                       ▼
     │   ┌────────────┐        ┌────────────┐
     │   │ service    │◄───────│ service    │  (same service node)
     │   └────────────┘        └────────────┘
     │ yielded
     ▼
   ┌────────────┐  exhibited   ┌────────────┐
   │ journal    │─────────────►│ deviation  │  (only when a run
   │ (run)      │              │            │   did not complete)
   └────────────┘              └────────────┘
```

### Node kinds

| Kind | Id shape | What it is |
|------|----------|------------|
| `experiment` | `exp:<title>` | The stable system-under-test identity, keyed by title. Recurs across runs. |
| `fault` | `fault:<plugin>::<function>` | A fault primitive (e.g. `tumult-net::inject_latency`). Shared by every run that injects it. Falls back to the injecting activity's name when only a journal is available. |
| `service` | `svc:<host>` | A service/target the experiment acts on (e.g. `demo-app`). Stable across runs. |
| `journal` | `run:<experiment_id>` | A single run's journal, labelled with its terminal status. Per run. |
| `deviation` | `dev:<experiment_id>` | Present only when a run did not complete cleanly. Per run. |

### Edge relations

| Relation | From → To | Meaning |
|----------|-----------|---------|
| `injects` | experiment → fault | the experiment injects this fault |
| `targets` | experiment → service | the experiment acts on this service |
| `yielded` | experiment → journal | this run of the experiment produced this journal |
| `observed_on` | fault → service | this fault was injected on this service |
| `exhibited` | journal → deviation | this run exhibited this deviation |
| `evidences` | experiment → compliance_article | a passing run supplies evidence toward this control (edge attrs carry citation strength) |
| `maps_to_compliance` | experiment → compliance_article | the experiment declares a mapping to this control (intent, not run-produced evidence) |
| `gap_in` | coverage_gap → fault_domain | an untested action is a gap in this plugin's fault domain |
| `depends_on` | service → service | declared runtime dependency, imported from a topology document — never derived from runs |
| `caused_by` | deviation → fault | the fault attributed as the cause of this deviation (only when attribution is unambiguous; see the [topology guide](topology.md)) |

`experiment`, `fault`, and `service` are stable identities that recur across runs; `journal` and `deviation` are per-run. That is what makes the graph accumulate history without duplicating structure: a hundred runs of the same experiment share one `exp:` node and one `fault:` node, and hang a hundred `run:` nodes off it.

## Arbitrary queries: `tumult_chaosgraph_cypher`

Beyond the fixed query shapes below, agents can run arbitrary **read-only
openCypher** over the whole graph. The graph is snapshotted from DuckDB (the
only source of truth) into an in-memory engine per call — disposable by
design. Mutating clauses are rejected before execution; rows are capped
(default 500).

```
MATCH (e:experiment)-[:injects]->(f:fault)-[:observed_on]->(s:service)
WHERE (other:service)-[:depends_on]->(s)
RETURN e.id, f.id, s.id
```

## The fixed-shape MCP tools

Both tools are read-only, idempotent, and return `structuredContent` with an advertised `outputSchema`. They read the persistent analytics store (`~/.tumult/analytics.duckdb` by default; override with `store_path`).

### `tumult_chaosgraph_query`

List node ids and one-line summaries for a kind, optionally filtered by a case-insensitive label substring.

Request:

```json
{ "kind": "fault", "filter": "latency" }
```

Structured response — `{kind, count, nodes:[{id,kind,label}]}`:

```json
{
  "kind": "fault",
  "count": 1,
  "nodes": [
    { "id": "fault:tumult-net::inject_latency", "kind": "fault", "label": "tumult-net::inject_latency" }
  ]
}
```

`kind` is one of `experiment`, `fault`, `service`, `journal`, `deviation`.

### `tumult_chaosgraph_neighbors`

Return a node's ego sub-graph: the nodes reachable within `depth` (following edges in both directions) plus the `(src)-[rel]->(dst)` tuples among them. Optionally filter to a single relation with `rel`. `depth` defaults to `1`.

Request:

```json
{ "node_id": "exp:Demo — network latency via the tumult-net userspace proxy", "depth": 1 }
```

Structured response — `{node_id, depth, nodes:[{id,kind,label}], edges:[{src,rel,dst}]}`:

```json
{
  "node_id": "exp:Demo — network latency via the tumult-net userspace proxy",
  "depth": 1,
  "nodes": [
    { "id": "exp:Demo — network latency via the tumult-net userspace proxy", "kind": "experiment", "label": "Demo — network latency via the tumult-net userspace proxy" },
    { "id": "fault:tumult-net::inject_latency", "kind": "fault", "label": "tumult-net::inject_latency" },
    { "id": "run:b1f2c3d4-5e6f-7a8b-9c0d-1e2f3a4b5c6d", "kind": "journal", "label": "completed" },
    { "id": "svc:demo-app", "kind": "service", "label": "demo-app" }
  ],
  "edges": [
    { "src": "exp:Demo — network latency via the tumult-net userspace proxy", "rel": "injects", "dst": "fault:tumult-net::inject_latency" },
    { "src": "exp:Demo — network latency via the tumult-net userspace proxy", "rel": "targets", "dst": "svc:demo-app" },
    { "src": "exp:Demo — network latency via the tumult-net userspace proxy", "rel": "yielded", "dst": "run:b1f2c3d4-5e6f-7a8b-9c0d-1e2f3a4b5c6d" },
    { "src": "fault:tumult-net::inject_latency", "rel": "observed_on", "dst": "svc:demo-app" }
  ]
}
```

Each tool also returns a compact text rendering alongside the structured content, e.g. `(exp:Demo …)-[injects]->(fault:tumult-net::inject_latency)`, and caps oversized output with a hint to narrow the query.

## How it populates on ingest

There is **no new service** and no separate build step. The graph lives in two DuckDB tables in the existing analytics store:

```
graph_nodes ( id PRIMARY KEY, kind, label, attrs JSON )
graph_edges ( src, rel, dst, run_id, ts )
```

These are added under **analytics schema v2** — an additive migration. The DDL uses `CREATE TABLE IF NOT EXISTS`, so opening a v1 store simply creates the two tables with no data loss; opening a fresh store creates everything at once.

Every path that ingests a journal populates the graph in the same transaction:

- `tumult run` and `tumult ingest` → journal-only mapping (faults keyed by the injecting activity's name).
- `tumult_run_experiment` over MCP → the richest mapping, because it has the experiment definition too: faults become `plugin::function` and services are extracted from the injecting activity's provider arguments.

Population is **dedup-safe**. Ingestion skips a journal whose `experiment_id` already exists, node upserts key on the primary id, and a run's edges are deleted by `run_id` before re-insert. Mapping the same run twice yields identical nodes and edges — re-ingesting never duplicates graph rows.

### One run, one journal node

Because `experiment`, `fault`, and `service` nodes are stable, each new run of an experiment adds exactly one `journal` node to that experiment's neighbourhood (plus a `deviation` node if the run did not complete cleanly).

```
run #1        exp ──yielded──► run:…a
run #2        exp ──yielded──► run:…a
                  └─yielded──► run:…b        (fault/service nodes unchanged)
run #3        exp ──yielded──► run:…a
                  ├─yielded──► run:…b
                  └─yielded──► run:…c
```

This is validated on the live demo: three consecutive runs grew the experiment's journal count 7 → 8 → 9 → 10, and `chaosgraph_query` / `chaosgraph_neighbors` returned the correct sub-graphs after each.

## Current scope (Phase 1) and limits

Phase 1 is deliberately narrow:

- **Service nodes and `targets`/`observed_on` edges are extracted only for some experiment shapes today.** A service is derived from a native provider's arguments — the first present of `upstream`, `target`, `host`, `service`, `endpoint`, `address`, `url`, `pod` — with the scheme, path, and `:port` stripped (so `http://demo-app:8080/health`, `demo-app:8080`, and `demo-app` all collapse to `demo-app`). Experiments that name their target differently, or that run through the journal-only path (no experiment definition), get faults but no service/target information yet.
- **No compliance or coverage overlay.** The graph does not yet carry framework verdicts, resilience scores, or coverage state on its nodes.
- **No blast-radius or path queries.** Neighbours expand a fixed-depth ego graph; there is no "shortest path between two services" or "everything within N hops of this fault" query surface beyond `depth`.

## Roadmap (Phase 2)

- **Generalised service extraction** across every provider shape, so `targets`/`observed_on` are populated for all experiments.
- **Compliance and coverage overlay** — attach framework verdicts and coverage state to nodes so an agent can ask "which services have untested faults?" straight from the graph.
- **Blast-radius and path queries** — richer traversals (path-between, radius) over the same node/edge model.

## See also

- [MCP Guide](mcp-guide.md) — the full 26-tool MCP surface, annotations, structured output, and resources.
- [Token Efficiency](token-efficiency.md) — why Tumult's data formats are cheap to feed to an LLM.
- [Analytics Guide](analytics-guide.md) — the DuckDB store the graph tables live in.
