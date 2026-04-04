# MCP over HTTP: Tumult as a Service

*2026-04-01*

Tumult's MCP server now supports HTTP/SSE transport, enabling any MCP-compatible
agent to connect over the network. This unlocks container-to-container
communication, multi-agent orchestration, and fleet-wide chaos engineering.

## Why HTTP?

The original stdio transport works for local use — MCP-compatible IDEs and agents connect directly. But for production agent fleets, containers,
and CI/CD pipelines, you need network transport.

The MCP 2025-11-25 specification defines **Streamable HTTP** — a protocol where
clients POST JSON-RPC requests and receive responses as Server-Sent Events (SSE).
This gives us:

- **Stateful sessions** — each client gets a session ID, enabling concurrent users
- **Streaming results** — long-running experiments can stream progress
- **Standard HTTP** — works through proxies, load balancers, firewalls
- **Session resumability** — clients can reconnect and resume

## Usage

```bash
# Local — stdio (default, for IDE integration)
tumult-mcp

# Network — HTTP/SSE (for containers and agent fleets)
tumult-mcp --transport http --port 3100

# Docker
docker run --network tumult-e2e -p 3100:3100 tumult-mcp
```

## Live Demo

All 14 MCP tools accessible over HTTP:

```
$ curl -s POST http://localhost:3100/mcp \
    -H "Accept: text/event-stream, application/json" \
    -d '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}'

Available tools: 14
  tumult_run_experiment
  tumult_validate
  tumult_analyze
  tumult_read_journal
  tumult_list_journals
  tumult_discover
  tumult_create_experiment
  tumult_query_traces
  tumult_store_stats
  tumult_analyze_store
  tumult_list_experiments
  tumult_gameday_run
  tumult_gameday_analyze
  tumult_gameday_list
```

Running a live experiment via HTTP:

```
$ curl POST http://localhost:3100/mcp \
    -d '{"method":"tools/call","params":{
      "name":"tumult_run_experiment",
      "arguments":{"experiment_path":"examples/postgres-failover.toon"}}}'

status: completed
duration_ms: 228
steady_state_before: met: true
steady_state_after:  met: true
full_recovery: true
mttr_s: 0
```

Querying the persistent analytics store:

```
$ curl POST http://localhost:3100/mcp \
    -d '{"method":"tools/call","params":{
      "name":"tumult_store_stats","arguments":{}}}'

store: analytics.duckdb
experiments: 67
activities: 244
size_mb: 2.51
```

## Architecture

```
┌──────────────────────┐     HTTP/SSE      ┌─────────────────┐
│  Coding Agent        │◄─────────────────►│  tumult-mcp     │
│  (AQE, IDEs,         │   :3100/mcp       │  --transport http│
│   Cursor, etc.)      │                   │                 │
└──────────────────────┘                   └────────┬────────┘
                                                    │
                                           ┌────────┴────────┐
                                           │  tumult-core    │
                                           │  10 plugins     │
                                           │  45 actions     │
                                           │  DuckDB store   │
                                           └─────────────────┘
```

## Docker Compose

The Tumult MCP container runs in HTTP mode by default:

```yaml
services:
  tumult-mcp:
    image: tumult-mcp:latest
    ports:
      - "3100:3100"
    environment:
      TUMULT_MCP_TOKEN: ${TUMULT_MCP_TOKEN:-tumult-dev}
    networks:
      - tumult-e2e
```

Any agent on the same Docker network can connect to `http://tumult-mcp:3100/mcp`.

## What This Enables

- **Agentic QE Fleet** — quality engineering agents run chaos experiments through Tumult
- **CI/CD chaos gates** — pipeline steps can call Tumult MCP to validate resilience
- **Multi-tenant** — multiple agents can share one Tumult instance via sessions
- **Remote chaos** — run experiments against production from a central control plane

Next: connecting the Agentic QE Fleet to Tumult for autonomous chaos engineering.
