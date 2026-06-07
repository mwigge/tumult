## Applied Toolkit Rules

Source: `/home/morgan/dev/src/agent-toolkit-bundle`.

Implementation work for this change follows these rules:

- Rust/TDD: write behavior tests before completing implementation slices; use typed `thiserror` errors in libraries; use `anyhow` only in binaries; avoid `.unwrap()` in library code; keep async external calls time-bound.
- AI developer: validate agent and tool outputs before using them; keep examples deterministic; use explicit token budgets in examples; avoid external provider calls in smoke tests.
- Chaos engineer: every scenario needs a steady-state expectation, realistic fault, blast-radius boundary, abort/reset guidance, and measurable contract outcome.
- Observability: emit or prepare OTel spans/records for agent scenario execution, fault application, contract evaluation, replay, scoring, and journal write; use structured logging only.
- Security/compliance: default to metadata-only capture; enforce target allowlists; redact sensitive evidence; do not log raw prompts, completions, retrieved documents, tool payloads, or secrets.
- CI/CD: fail fast through smoke tests, crate tests, formatting, clippy, workspace tests, dependency audit, and OpenSpec validation.

## Smoke-Test Plan

Smoke tests are the first feedback loop for every implementation slice. They must run offline and avoid external LLM providers, network services outside local test fixtures, and secrets.

### Local Fake HTTP Agent

- Target adapter: `http`
- Scenario: `fake-http-malformed-json`
- Fault: `malformed_output`
- Contract: `valid_json`
- Expected result: contract fails, score is `0.0`, journal contains target type, scenario name, fault type, contract outcome, trace ID, and no raw input or completion fields.
- Failure output must include adapter, scenario, fault, contract, expected value, actual value, and next diagnostic command.

### Local Fake MCP Target

- Target adapter: `mcp`
- Scenario: `fake-mcp-tool-timeout`
- Fault: `tool_failure` with low-cardinality `timeout`
- Contract: `fallback_used`
- Expected result: fallback contract passes when the fake agent reports fallback, and fails clearly otherwise.

### Replay Fixture

- Target adapter: `replay`
- Scenario: `replay-malformed-tool-result`
- Fixture: normalized local steps for model response and tool result.
- Expected result: replay runs deterministically, missing step outputs fail validation before execution.

### Required Smoke Output

Passing smoke output must show:

- adapter exercised
- scenario name
- fault type
- contracts evaluated
- score
- journal path or encoded journal summary
- trace assertion summary
- confirmation that external network/provider calls were not used

Failing smoke output must show:

- expected value
- actual value
- scenario name
- fault type
- contract name
- next diagnostic command
