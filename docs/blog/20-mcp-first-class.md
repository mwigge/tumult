---
title: "Your Agent Is Now a First-Class Tumult Operator"
parent: Blog
nav_order: 21
updated: 2026-07-21
---

# Your Agent Is Now a First-Class Tumult Operator

*2026-07-04*

Since 1.0, Tumult has had an MCP server. It worked. It also treated the agent on the other end like a tourist: 19 tools, all returning prose, no way to tell a harmless read from a fault injection, and an experiment runner that threw its journal away the moment the call returned.

Version 2.1.0 rebuilt that surface around the MCP specification. It shipped 24 tools, annotations on every tool, advertised structured-output schemas on 16, workspace files as MCP resources, and a complete run-to-analyze-to-recommend loop.

Let's take those one at a time.

## The loop is closed

The previous behavior had a concrete gap. `tumult_run_experiment` executed an experiment and rendered the journal into the response, but wrote and ingested nothing. The analytics store that powers `recommend`, `coverage`, and `trend` never received the run.

```
before 2.1.0:

  agent ──► run_experiment ──► journal text ──► (gone)

  recommend / coverage / trend ──► store ──► "you've never tested anything"
                                              (the store never saw the runs)
```

The tool was a dead-end executor. An agent could run chaos all afternoon and the intelligence tools would keep recommending the experiments it had just finished.

Now `tumult_run_experiment` does what `tumult run` has always done: it persists the journal (`journal_path`, default `journal.toon`) and auto-ingests it into the analytics store. Skip with `no_ingest`, redirect with `store_path`; CLI parity, parameter for parameter.

```
2.1.0:

  agent ──► run_experiment ──┬──► journal.toon written
                             └──► ingested into analytics store
                                        │
              ┌─────────────────────────┼─────────────────────┐
              ▼                         ▼                     ▼
        tumult_recommend         tumult_coverage        tumult_trend
        "test kafka next"        "tumult-net: NONE"     "score improving"
              │
              ▼
        agent picks the gap ──► run_experiment ──► (loop)
```

Run, learn, decide, run again. That's chaos engineering. Now it's chaos engineering an agent can do alone in a room.

The result even tells you how the ingestion went; `ingested`, `duplicate`, `skipped`, or `failed: <reason>`. Ingestion trouble is a warning in the result, never a failed run.

## Annotations: reads are free, chaos is gated

MCP tool annotations exist so a client can make policy decisions *before* calling anything. Every one of the 24 tools now declares its `readOnlyHint` / `destructiveHint` / `idempotentHint` / `openWorldHint`.

The split is stark, and that's the point:

```
                     24 tools
                        │
        ┌───────────────┼──────────────────┐
        ▼               ▼                  ▼
  18 read-only     2 destructive      4 writers
  idempotent       + open-world       (non-destructive)
                                       
  validate          run_experiment     create_experiment
  read_journal      gameday_run        gameday_create
  analyze                              report
  compliance        these inject       recommend
  trend             REAL faults        (open-world with
  coverage          into REAL          agent= set)
  ...               systems

  client policy:    client policy:     client policy:
  auto-approve ✓    ASK THE HUMAN ⚠    approve writes ✎
```

A well-behaved MCP client reads these hints and does the obvious thing: let the agent grep through journals, compliance verdicts, and coverage reports without ceremony; and put a human approval gate in front of the two tools that pause your Redis or kill your database connections.

One annotation worth calling out: `tumult_recommend` is marked `openWorldHint: true`, but not because Tumult phones home. When you pass `agent: claude-code`, the tool spawns your local agent CLI, and *that* may call its model API. The annotation tells the truth about the blast radius.

## Structured output: schemas, not prose

The old tools returned text. Nicely formatted text! Which your agent then... parsed. With regexes. In 2026.

```
before:                              after:

┌─────────────────────┐    ┌──────────────────────────────┐
│ "Experiment passed!  │    │ content: [ text (unchanged) ] │
│  Duration: 314ms.    │    │ structuredContent: {          │
│  3 steps executed,   │    │   journal: {...},             │
│  journal looks OK."  │    │   journal_path: "...",        │
└─────────┬───────────┘    │   ingestion: "ingested"       │
          │                └──────────────┬───────────────┘
          ▼                               │
  agent regexes the prose      validated against the
  and hopes for the best       outputSchema advertised
                               in tools/list ✓
```

16 of the 24 tools now return `structuredContent` and advertise a matching `outputSchema`. Here's a real one; the shape `tumult_run_experiment` declares, filled in by the Redis experiment from the README:

```json
{
  "journal": {
    "experiment_title": "Redis resilience; verify recovery after disruption",
    "experiment_id": "d2f8...",
    "status": "completed",
    "started_at_ns": 1782043200000000000,
    "ended_at_ns": 1782043200314000000,
    "duration_ms": 314,
    "method_results": [
      {
        "name": "pause-redis",
        "activity_type": "action",
        "status": "succeeded",
        "duration_ms": 102,
        "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
        "span_id": "00f067aa0ba902b7"
      }
    ],
    "rollback_results": []
  },
  "journal_path": "/workspace/journal.toon",
  "ingestion": "ingested"
}
```

The `status` field is an enum in the schema (`completed` / `deviated` / `aborted` / `failed` / `interrupted`), the activity statuses are enums, the trace IDs are right there for your observability stack. No prose archaeology.

The same strictness applies on the way in: enum-ish parameters (`format`, `rollback_strategy`, `framework`, `metric`, `load_tool`) reject unknown values with the list of valid ones. And every inline text payload is capped at 512 KiB with an explicit truncation notice; your context window will thank us.

## Resources: the workspace has URIs now

Tool calls aren't the only way to get at files anymore. The workspace is served under a proper URI scheme:

```
tumult://
   │
   ├── journal/{file} ──────── journals, read as JSON {summary, journal}
   │                           (application/json)
   ├── experiment/{file} ───── experiment definitions, raw TOON
   │                           (application/toon)
   └── gameday/{file} ──────── campaign files, raw TOON
                               (application/toon)

  and tools hand out links into it:

  run_experiment ────► resource_link: tumult://journal/journal.toon
  gameday_create ────► resource_link: tumult://gameday/drill.gameday.toon
  report ────────────► resource_link: tumult://journal/report.xml's path
  list_journals ─────► one resource_link per journal (first 50)
```

An MCP client receives text, structured content, and a link to any artifact the tool produced. It can follow that link with `resources/read` without guessing a path. `resources/list` paginates with opaque cursors in pages of 100, and the three list tools accept `limit`, `offset`, and `total` so a response remains bounded.

Filenames only, by the way; path separators and traversal attempts in a resource URI are rejected with the same containment checks the tools use. And if you've set `TUMULT_MCP_TOKEN`, resource requests pass the same bearer gate as tool calls.

## The full map

Five of the 24 are new in 2.1.0 (`report`, `compliance`, `trend`, `agents`, `gameday_create`), and together the surface now covers the whole workflow:

```
 discover            validate           run                analyze
 ────────            ────────           ───                ───────
 discover            validate           run_experiment ⚠   read_journal
 list_experiments    create_experiment                     list_journals
 agents                                                    analyze
                                                           analyze_store
                                                           store_stats
                                                           query_traces

 report              compliance         gameday            recommend
 ──────              ──────────         ───────            ─────────
 report              compliance         gameday_create     recommend
 trend                                  gameday_run ⚠      coverage
                                        gameday_analyze
 agentic: list_scenarios,               gameday_list       ⚠ = destructive,
          smoke, run_experiment                                gate it
```

Everything an operator does at the terminal, an agent can now do over MCP; with the same shared code underneath. `tumult_compliance` runs the same `tumult_core::compliance` scoring as `tumult compliance`. `tumult_recommend` runs the same `tumult-intelligence` engine as the CLI, agent enhancement and validation gate included. `tumult_gameday_run` even fixed a real bug on the way in: it used to silently skip the campaign's declared load; it now drives the same k6 executor as the CLI.

One implementation, two front doors.

## Try it

```bash
tumult-mcp --transport http --port 3100
```

Point any MCP client at it, list the tools, and watch it read the annotations before it touches anything. Then let it run an experiment and ask for a recommendation; the answer will already know about the run.

---

*The MCP server negotiates protocol revision `2025-11-25` and ships in 2.1.0. The [MCP Guide](../guides/mcp-guide.md) has the full data model; annotations, schemas, resources, pagination, and auth.*
