---
title: Agentic Scenarios
parent: Guides
nav_order: 10
---

# Agentic Scenarios

Agentic scenario packs combine a target adapter, one or more faults, and
behavioral contracts. They are meant for repeatable local and CI checks before
running broader experiments against real agents.

## Bundled Packs

List the currently bundled packs:

```bash
tumult agentic list-packs
```

Current bundled packs:

| Pack | Adapters | Faults | Required contracts | Use |
|---|---|---|---|---|
| `concurrency-storm` | HTTP, MCP, replay | `rate_limit`, `retry_loop_pressure` | `max_latency`, `retry_budget`, `graceful_error` | Verifies retry behavior and latency under pressure |
| `hallucination-under-timeout` | HTTP, MCP, replay | `model_timeout`, `hallucinated_tool_call` | `max_tool_calls`, `graceful_error`, `fallback_used` | Verifies agents do not invent unsafe tool paths during timeout recovery |
| `cost-explosion-detector` | HTTP, replay | `token_budget_exhaustion`, `retry_loop_pressure` | `max_token_usage`, `retry_budget` | Verifies token and retry budgets cap runaway cost |
| `malformed-json-recovery` | HTTP, replay | `malformed_output` | `valid_json`, `graceful_error` | Verifies that structured-output agents recover from invalid model output |
| `tool-timeout-fallback` | MCP, replay | `tool_latency`, `tool_failure` | `fallback_used`, `max_latency` | Verifies fallback behavior when a tool call cannot complete |
| `retrieval-poisoning` | HTTP, replay | `retrieval_poisoning` | `required_citation`, `no_pii`, `no_secret_leakage` | Verifies retrieval corruption does not leak unsafe content |

## Fault Types

Supported fault types:

- `model_latency`
- `model_timeout`
- `provider_error`
- `rate_limit`
- `malformed_output`
- `output_truncation`
- `hallucinated_tool_call`
- `tool_latency`
- `tool_failure`
- `retrieval_poisoning`
- `context_truncation`
- `token_budget_exhaustion`
- `retry_loop_pressure`

## Contract Types

Supported contract types:

- `valid_json`
- `required_citation`
- `no_pii`
- `no_secret_leakage`
- `max_latency`
- `retry_budget`
- `max_tool_calls`
- `max_token_usage`
- `fallback_used`
- `graceful_error`

## Target Adapters

The model supports HTTP, MCP, and replay targets:

```json
{ "type": "http", "endpoint": "http://127.0.0.1:8080/agent" }
{ "type": "mcp", "server": "local", "tool": "answer_question" }
{ "type": "replay", "fixture": "examples/agentic/malformed-json-recovery.fixture.json" }
```

Targets must pass allowlist validation before invocation. Smoke tests use fake
local adapters or replay fixtures and do not call external providers.

## Deterministic Runs

The first implementation executes bundled packs through local fixtures:

```bash
tumult agentic run --scenario malformed-json-recovery
tumult agentic run --scenario cost-explosion-detector
tumult agentic replay --fixture examples/agentic/malformed-json-recovery.fixture.json
```

Each command writes a metadata-only journal and prints trace assertions. Future
provider/framework adapters can reuse the same fault, contract, journal, and
analytics model.
