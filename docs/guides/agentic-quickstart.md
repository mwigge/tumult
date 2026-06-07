---
title: Agentic Fault Injection Quickstart
parent: Guides
nav_order: 8
---

# Agentic Fault Injection Quickstart

Tumult agentic fault injection tests how an AI agent behaves when model output,
tool calls, retrieval, context, or retry behavior becomes slow, partial, or
wrong. The local smoke path is deterministic and does not call an external LLM,
network endpoint, MCP server, or secret store.

The module is implemented in `tumult-agentic` and is surfaced through the
`tumult agentic` CLI group.

## List Scenario Packs

```bash
tumult agentic list-packs
```

Expected output includes bundled packs and their fault/contract matrix:

```text
Agentic scenario packs
count: 6

- concurrency-storm
  adapters: http, mcp, replay
  faults: rate_limit, retry_loop_pressure
  contracts: max_latency, retry_budget, graceful_error

- hallucination-under-timeout
  adapters: http, mcp, replay
  faults: model_timeout, hallucinated_tool_call
  contracts: max_tool_calls, graceful_error, fallback_used

- cost-explosion-detector
  adapters: http, replay
  faults: token_budget_exhaustion, retry_loop_pressure
  contracts: max_token_usage, retry_budget

- malformed-json-recovery
  adapters: http, replay
  faults: malformed_output
  contracts: valid_json, graceful_error

- tool-timeout-fallback
  adapters: mcp, replay
  faults: tool_latency, tool_failure
  contracts: fallback_used, max_latency

- retrieval-poisoning
  adapters: http, replay
  faults: retrieval_poisoning
  contracts: required_citation, no_pii, no_secret_leakage
```

See [Agentic Scenarios](agentic-scenarios.md) for the full fault and contract
catalogue.

## Run the Local Smoke Path

```bash
tumult agentic smoke
tumult agentic run --scenario cost-explosion-detector
tumult agentic replay --fixture examples/agentic/malformed-json-recovery.fixture.json
```

The smoke path injects malformed JSON into a fake HTTP agent response and
expects the `valid_json` contract to fail. That failure is the success signal:
the fault was applied, the contract detected it, and the CLI reported the
feedback loop clearly.

Expected passing output:

```text
Agentic smoke: malformed-json-recovery
adapter: fake_http
target_type: http
scenario: fake-http-malformed-json
fault: malformed_output
fault_applied: true
contract: valid_json
expected_contract: contract_failed:invalid_json
actual_contract: contract_failed:invalid_json
reason: invalid_json
resilience_score: 0.000
trace_id: trace-smoke-http
journal: target/agentic/smoke-journal.toon
analytics_store: /home/morgan/.tumult/analytics.duckdb
analytics_ingested: true
trace_assertions: trace_id_present=true span_id_present=true capture_policy=metadata_only
network: not required
next: cargo test -p tumult-agentic smoke_fake_http -- --nocapture
result: pass (fault observed and contract feedback captured)
```

If the smoke path fails, the output names the adapter, scenario, fault, contract,
expected value, actual value, and next diagnostic command. Use that output as the
first feedback loop before running broader crate or workspace tests.

## Observe the Run

The smoke path is local, but it still reports the trace correlation fields that
real HTTP, MCP, and replay adapters should preserve. Agentic telemetry uses both
Tumult resilience attributes and GenAI semantic attributes:

```text
resilience.agent.scenario=malformed-json-recovery
resilience.agent.fault.type=malformed_output
resilience.agent.payload.capture_policy=metadata_only
gen_ai.operation.name=invoke_agent
gen_ai.evaluation.result=fail
```

See [Agentic Observability](agentic-observability.md) for the full attribute
reference and privacy defaults.

## Fixture Example

A minimal provider-free fixture lives at
`examples/agentic/malformed-json-recovery.fixture.json`. It models the same
malformed structured-response case used by the smoke path and can be extended
for replay regression work.

## Analytics

Agentic smoke, run, and replay commands write metadata-only rows into the
default analytics store when available:

- `agentic_runs`
- `agentic_contract_outcomes`
- `agentic_fault_applications`
- `agentic_replay_outcomes`

Repeated deterministic runs may report `analytics_ingested: false` when the
same run ID already exists; this is duplicate protection, not a failed smoke.

## Quality Gate

Use the narrow smoke loop first:

```bash
cargo test -p tumult-cli parse_agentic
cargo run -p tumult-cli -- agentic list-packs
cargo run -p tumult-cli -- agentic smoke
cargo run -p tumult-cli -- agentic run --scenario cost-explosion-detector
cargo run -p tumult-cli -- agentic replay --fixture examples/agentic/malformed-json-recovery.fixture.json
```

Then run the broader agentic gate:

```bash
cargo test -p tumult-agentic
cargo fmt --check
cargo clippy -p tumult-agentic -- -D warnings -W clippy::pedantic
```
