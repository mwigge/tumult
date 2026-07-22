---
title: "ChaosGraph: Your Agent Stops Re-Reading Journals"
parent: Blog
nav_order: 21
updated: 2026-07-21
---

# ChaosGraph: Your Agent Stops Re-Reading Journals

*2026-07-04*

An assistant may need one fact, such as which fault an experiment injects and which service it targets, but reading the journal loads every hypothesis, method result, provider output, trace identifier, and analysis record. That is a large context cost for four relationships.

2.4.0 ships ChaosGraph to fix exactly that. It's a typed knowledge graph over your chaos data, built from journals as they ingest, and served to agents over MCP as compact sub-graphs. The agent reads relationships, not records.

## Four kinds of thing, five kinds of arrow

ChaosGraph is small on purpose. Every run collapses to a handful of typed nodes and edges.

```
                 injects
   ┌────────────┐──────────►┌──────────────────────────┐
   │ experiment │           │ fault (plugin::function) │
   └────────────┘           └──────────────────────────┘
      │      │                        │
      │      │ targets                │ observed_on
      │      ▼                        ▼
      │   ┌───────────┐         ┌───────────┐
      │   │ service   │◄────────│ service   │   (same node)
      │   └───────────┘         └───────────┘
      │ yielded
      ▼
   ┌────────────┐   exhibited   ┌────────────┐
   │ journal    │──────────────►│ deviation  │   (only if the run
   │ (run)      │               │            │    didn't complete)
   └────────────┘               └────────────┘
```

The original graph used five node kinds (`experiment`, `fault`, `service`, `journal`, and `deviation`) and five edge relations (`injects`, `targets`, `yielded`, `observed_on`, and `exhibited`). Later releases extend that model; consult the ChaosGraph guide for the current schema.

The trick is which nodes are *stable*. `experiment`, `fault`, and `service` recur across runs; a hundred latency drills share one experiment node and one `tumult-net::inject_latency` fault node. Only `journal` and `deviation` are per-run. So the graph accumulates history without ever duplicating structure.

## The token math

This is the part that matters for anyone paying for context.

`make demo-proof` reproduces the measurements against the demo. In that proof, a filtered `chaosgraph_neighbors` response is about **110 tokens**, while a raw journal is about **480 tokens**. The targeted graph response remains bounded as runs accumulate. The saved proof also measured roughly **8×** less context per run of history and about **20×** less for its store-wide query. These figures describe that fixture, not a universal compression ratio.

```
before:  every run adds a full journal to what you'd read

  agent ──► read_journal (xN) ──► ┌───────────────────────────────┐
                                  │ ~480 tokens EACH, and it grows │
                                  │  by one more every run:        │
                                  │  hypothesis, method_results,   │
                                  │  provider output, trace/span…  │
                                  └───────────────────────────────┘

after:  a targeted query stays the same size forever

  agent ──► chaosgraph_neighbors ──► ┌────────────────────────┐
                                     │ ~110 tokens, bounded:  │
                                     │  (exp)-[injects]->(…)  │
                                     │  (exp)-[targets]->(…)  │
                                     │  (exp)-[yielded]->(…)  │
                                     │  (fault)-[observed]->  │
                                     └────────────────────────┘
```

Same information. One reads like a database dump; the other reads like an answer.

## Two tools, both read-only

The whole surface is two MCP tools (they took the count from 24 to 26).

`tumult_chaosgraph_query` lists nodes of a kind; "show me every fault", "every service"; with an optional label filter.

`tumult_chaosgraph_neighbors` is the good one. Give it a node and it hands back the ego sub-graph: everything within `depth` (default 1), as `(src)-[rel]->(dst)` tuples plus labels. A representative result; the demo's network-latency experiment, centred, depth 1:

```json
{
  "node_id": "exp:Demo; network latency via the tumult-net userspace proxy",
  "depth": 1,
  "nodes": [
    { "id": "exp:Demo; network latency via the tumult-net userspace proxy", "kind": "experiment", "label": "Demo; network latency via the tumult-net userspace proxy" },
    { "id": "fault:tumult-net::inject_latency", "kind": "fault", "label": "tumult-net::inject_latency" },
    { "id": "run:b1f2c3d4-5e6f-7a8b-9c0d-1e2f3a4b5c6d", "kind": "journal", "label": "completed" },
    { "id": "svc:demo-app", "kind": "service", "label": "demo-app" }
  ],
  "edges": [
    { "src": "exp:Demo; network latency via the tumult-net userspace proxy", "rel": "injects", "dst": "fault:tumult-net::inject_latency" },
    { "src": "exp:Demo; network latency via the tumult-net userspace proxy", "rel": "targets", "dst": "svc:demo-app" },
    { "src": "exp:Demo; network latency via the tumult-net userspace proxy", "rel": "yielded", "dst": "run:b1f2c3d4-5e6f-7a8b-9c0d-1e2f3a4b5c6d" },
    { "src": "fault:tumult-net::inject_latency", "rel": "observed_on", "dst": "svc:demo-app" }
  ]
}
```

Both tools are read-only and idempotent, both return `structuredContent` with an advertised `outputSchema`, so a client validates the shape instead of parsing prose.

## It builds itself, one run at a time

No new service. No separate build step. The graph is two DuckDB tables; `graph_nodes` and `graph_edges`; added to the existing analytics store under schema v2. The migration is additive (`CREATE TABLE IF NOT EXISTS`), so an old store just grows two empty tables the next time it opens. The single binary is still a single binary.

Every journal that ingests populates the graph in the same breath. Run over MCP with the experiment definition in hand and you get the rich mapping; `fault = plugin::function`, services pulled from the provider's arguments. Ingest a bare journal from the CLI and you still get faults, keyed by the injecting activity's name.

And because the recurring nodes are stable, each new run drops exactly *one* journal node onto the experiment's neighbourhood:

```
run #1     exp ──yielded──► run:…a

run #2     exp ──yielded──► run:…a
               └─yielded──► run:…b          (fault + service nodes: untouched)

run #3     exp ──yielded──► run:…a
               ├─yielded──► run:…b
               └─yielded──► run:…c
```

We watched this happen on the live demo: three experiments in a row, the journal count climbing 7 → 8 → 9 → 10, and `chaosgraph_query` / `chaosgraph_neighbors` returning the right sub-graph after every one. It's dedup-safe too; re-ingesting the same run upserts nodes and rewrites that run's edges, so nothing ever doubles up.

## What it isn't yet

The limits of Phase 1: service nodes and the `targets` / `observed_on` edges only come out for experiments whose provider arguments name a target the extractor recognises (`upstream`, `target`, `host`, `service`, and a few more; scheme, path, and port stripped off). Journal-only ingests get faults but no service. There's no compliance or coverage data on the nodes yet, and no blast-radius or shortest-path queries beyond expanding a fixed depth.

Phase 2 is where that opens up: generalised service extraction across every provider shape, a compliance/coverage overlay so an agent can ask "which services have untested faults?" straight from the graph, and real blast-radius and path traversals.

## Try it

Run an experiment over MCP, then ask the graph what happened:

```bash
tumult-mcp --transport http --port 3100
```

Call `tumult_run_experiment`, then `tumult_chaosgraph_neighbors` on the experiment node. Seventy tokens, four relationships, done; no journal archaeology required.

---

*ChaosGraph ships in 2.4.0 as the in-workspace `tumult-graph` crate plus two DuckDB tables. The [ChaosGraph guide](../guides/chaosgraph.md) has the full node/edge model and both tools' request/response shapes; the [MCP Guide](../guides/mcp-guide.md) covers the current tool surface it plugs into.*
