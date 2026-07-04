---
title: MCP Guide
parent: Guides
nav_order: 12
---

# MCP Guide

Tumult ships a built-in [Model Context Protocol](https://modelcontextprotocol.io/) server, `tumult-mcp`, that gives AI assistants and agent fleets the full chaos engineering workflow: discover, author, validate, run, analyze, report, prove compliance, orchestrate GameDays, and decide what to test next — all without shelling out to the CLI.

The server negotiates MCP protocol revision `2025-11-25` and exposes **26 tools** plus workspace **resources** under the `tumult://` URI scheme.

## Starting the server

```bash
tumult-mcp                                # stdio (IDE integration)
tumult-mcp --transport http --port 3100   # HTTP/SSE (containers, agent fleets, CI/CD)
TUMULT_MCP_TOKEN=my-secret tumult-mcp     # with bearer token auth
```

All file arguments are resolved against the server's workspace root; paths that escape it are rejected.

## The closed feedback loop

`tumult_run_experiment` doesn't just return the journal — it **persists** it (`journal_path`, default `journal.toon`, CLI parity) and **auto-ingests** it into the persistent analytics store (skip with `no_ingest`, point elsewhere with `store_path`). The result reports the ingestion outcome: `ingested`, `duplicate`, `skipped`, or `failed: <reason>` (ingestion failures are warnings, not run failures).

That closes the loop over MCP alone:

```
run_experiment ──► journal written + ingested ──► store
                                                    │
        ┌───────────────────────────────────────────┤
        ▼                    ▼                      ▼
   tumult_recommend    tumult_coverage        tumult_trend
        │
        ▼
   next experiment ──► run_experiment ──► ...
```

An agent can run an experiment and immediately see the result reflected in recommendations, coverage, and trend — the same loop `tumult run` + `tumult recommend` gives a human at the terminal.

## The 26 tools, by workflow stage

### Discover & author

| Tool | Notes |
|------|-------|
| `tumult_discover` | All plugins, actions, and probes (script + native) |
| `tumult_create_experiment` | New experiment from a template |
| `tumult_validate` | Syntax + provider support check |
| `tumult_list_experiments` | Recursive `.toon` listing, paginated |

### Run

| Tool | Notes |
|------|-------|
| `tumult_run_experiment` | Executes, persists the journal, auto-ingests. Params: `experiment_path`, `rollback_strategy` (`on-deviation` / `always` / `never`), `journal_path`, `no_ingest`, `store_path`, `format` (`json` / `toon`) |

### Journals & analysis

| Tool | Notes |
|------|-------|
| `tumult_read_journal` | JSON by default (`format=toon` for raw TOON), `summary=true` for a compact summary |
| `tumult_list_journals` | Paginated; attaches `resource_link`s (first 50) |
| `tumult_analyze` | SQL over journal files via embedded DuckDB (SELECT-only guard) |
| `tumult_analyze_store` | SQL over the persistent analytics store |
| `tumult_store_stats` | Experiment/activity counts, schema version, file size |
| `tumult_query_traces` | Activity spans with trace/span IDs for observability correlation |
| `tumult_trend` | Metric trend over runs: `resilience_score` (default), `duration_ms`, `estimate_accuracy`, `method_step_count`; optional `last` window (e.g. `30d`) and `target` title filter; returns time-ordered points and a direction verdict |

### Report & compliance

| Tool | Notes |
|------|-------|
| `tumult_report` | `format=json` (raw journal) or `junit` (one `<testcase>` per activity); inline (capped at 512 KiB) or written via `output_path`. HTML/PDF stay CLI-only |
| `tumult_compliance` | Pass rate, recovery compliance, and COMPLIANT / PARTIAL / NON-COMPLIANT verdict for one of seven frameworks: `dora`, `nis2`, `pci-dss`, `iso-22301`, `iso-27001`, `soc2`, `basel-iii`. Shares `tumult_core::compliance` with the CLI |

### GameDay

| Tool | Notes |
|------|-------|
| `tumult_gameday_create` | Scaffolds `<name>.gameday.toon` from experiment paths, optional shared load (`load_tool` k6/jmeter, `load_script`, `load_vus`) and framework mapping; refuses to overwrite |
| `tumult_gameday_run` | Runs the campaign — including its declared shared load, through the same k6 executor as `tumult gameday run` |
| `tumult_gameday_analyze` | Resilience score, per-experiment results, compliance article mapping |
| `tumult_gameday_list` | Paginated `.gameday.toon` listing |

### Intelligence

| Tool | Notes |
|------|-------|
| `tumult_recommend` | Deterministic heuristics from `tumult-intelligence` (shared with the CLI); optional agent enhancement via `agent` (`claude-code` / `codex`), `agent_model`, `agent_timeout_secs`, `generate_experiments_dir`. Agent-proposed experiments pass a parse + validate gate before being written |
| `tumult_coverage` | Per-plugin FULL / PARTIAL / NONE test status plus store statistics |
| `tumult_agents` | Detected agent CLI adapters: installed, version, auth state |

### Agentic AI

| Tool | Notes |
|------|-------|
| `tumult_agentic_list_scenarios` | Scenario pack metadata (no prompts or raw payloads) |
| `tumult_agentic_smoke` | Deterministic local smoke check, metadata-only |
| `tumult_agentic_run_experiment` | Bundled agentic experiment with input schema validation |

### ChaosGraph

| Tool | Notes |
|------|-------|
| `tumult_chaosgraph_query` | Node ids + one-line summaries for a `kind` (`experiment`, `fault`, `service`, `journal`, `deviation`), optional case-insensitive label `filter`. Structured: `{kind, count, nodes:[{id,kind,label}]}` |
| `tumult_chaosgraph_neighbors` | A node's ego sub-graph as compact `(src)-[rel]->(dst)` tuples plus labels, within `depth` (default 1), optional `rel` filter. Structured: `{node_id, depth, nodes, edges}` |

These serve compact sub-graphs instead of whole journals — roughly **37× fewer tokens** for "what did this experiment touch?". See the [ChaosGraph guide](chaosgraph.md) for the node/edge model, request/response examples, and the roadmap.

## Tool annotations

Every tool declares the MCP annotation hints — `readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint` — so a client can make policy decisions before calling anything:

| Class | Count | Tools |
|-------|-------|-------|
| Read-only, idempotent | 20 | `validate`, `analyze`, `read_journal`, `list_journals`, `discover`, `query_traces`, `store_stats`, `analyze_store`, `list_experiments`, `compliance`, `trend`, `agents`, `gameday_analyze`, `gameday_list`, `coverage`, `agentic_list_scenarios`, `agentic_smoke`, `agentic_run_experiment`, `chaosgraph_query`, `chaosgraph_neighbors` |
| Destructive + open-world | 2 | `run_experiment`, `gameday_run` — these inject real faults into real systems |
| Non-destructive writers | 4 | `create_experiment`, `gameday_create` (refuses overwrite), `report` (idempotent), `recommend` (open-world when `agent` is set: the local agent CLI may reach its model API, and validated experiments are written to disk) |

A well-behaved MCP client can auto-approve the 20 read-only tools and require explicit human approval for the two destructive ones. That is the intended contract: reads are free, chaos is gated.

## Structured output

18 tools return `structuredContent` alongside their human-readable text, and advertise a matching `outputSchema` in `tools/list` so clients can validate results mechanically:

`run_experiment`, `read_journal`, `report`, `compliance`, `trend`, `gameday_create`, `agents`, `recommend`, `store_stats`, `coverage`, `agentic_list_scenarios`, `agentic_smoke`, `agentic_run_experiment`, `list_journals`, `list_experiments`, `gameday_list`, `chaosgraph_query`, `chaosgraph_neighbors`.

Example — `tumult_run_experiment` structured content (shape per its advertised schema):

```json
{
  "journal": {
    "experiment_title": "Redis resilience — verify recovery after disruption",
    "experiment_id": "d2f8...",
    "status": "completed",
    "started_at_ns": 1782043200000000000,
    "ended_at_ns": 1782043200314000000,
    "duration_ms": 314,
    "method_results": [ { "name": "pause-redis", "activity_type": "action", "status": "succeeded", "...": "..." } ],
    "rollback_results": []
  },
  "journal_path": "/workspace/journal.toon",
  "ingestion": "ingested"
}
```

Other contracts worth knowing:

- **Journals as JSON.** `tumult_read_journal` and `tumult_run_experiment` return the journal as JSON by default; `format=toon` gets you the raw TOON.
- **Strict enums.** Parameters with a fixed value set (`format`, `rollback_strategy`, `framework`, `metric`, `load_tool`) reject unknown values with an error listing the valid ones — no silent defaults.
- **512 KiB cap.** Inline text content is capped at 512 KiB with an explicit truncation notice appended.
- **`isError: true`** on tool failures per the spec; auth and rate-limit rejections surface as such, not as "Unknown tool".

## Resources

Workspace files are addressable as MCP resources (filenames only — path separators and traversal are rejected):

```
tumult://
├── journal/{file}       journal .toon files — read as JSON {summary, journal}
│                        (over 512 KiB: summary plus a note)   application/json
├── experiment/{file}    experiment definitions — raw TOON     application/toon
└── gameday/{file}       .gameday.toon campaigns — raw TOON    application/toon
```

`resources/list` enumerates the workspace root and paginates with opaque cursors in pages of 100.

Tools hand out `resource_link` content items pointing into this scheme: `tumult_run_experiment` links the journal it wrote, `tumult_gameday_create` the campaign file, `tumult_report` (with `output_path`) the written report, and `tumult_list_journals` one link per journal (first 50). A client can follow a link with `resources/read` instead of re-invoking a tool.

## Pagination

Two mechanisms, matching the two MCP surfaces:

- **List tools** (`tumult_list_journals`, `tumult_list_experiments`, `tumult_gameday_list`): `limit` (default 100, max 1000) and `offset` parameters; structured content is `{items, total, offset, limit}`.
- **`resources/list`**: spec-standard opaque `cursor` / `nextCursor`, pages of 100. Invalid cursors are protocol errors.

## Authentication & limits

Set `TUMULT_MCP_TOKEN` to require bearer token authentication. Both tool calls and resource requests are gated; clients pass the token via `_meta.authorization` (stdio has no HTTP header context). Comparison is constant-time (`subtle` crate), and a Semaphore(10) rate-limits concurrent calls. If the token is unset, the server runs open and logs a warning.

## See also

- [CLI Reference — tumult-mcp](cli-reference.md#tumult-mcp) for the flat tool table
- [Agentic Recommendations](agentic-recommendations.md) for the `recommend --agent` flow the MCP tool mirrors
- [Agentic Quickstart](agentic-quickstart.md) for the agentic fault-injection tools
