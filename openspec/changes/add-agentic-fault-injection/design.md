## Context

Tumult already has the primitives needed for agentic resilience testing: a five-phase experiment model, TOON experiments and journals, script/native plugin execution, MCP integration, OpenTelemetry trace context propagation, DuckDB/Parquet analytics, and `resilience.*` telemetry attributes. The missing boundary is that AI agents are currently callers of Tumult, not systems under test.

The market scan shows two direct patterns worth adopting: Flakestorm's contracts x chaos matrix and replay regression model, and Chaosync's low-friction scenario packs and framework-oriented runner model. Tumult should apply those lessons through its existing strengths: Rust-native execution, OTel-native evidence, infra + AI-layer chaos in one experiment, and structured journal/analytics output.

The relevant OpenTelemetry GenAI conventions are still marked development, so Tumult should use them as integration attributes and events while keeping stable Tumult experiment metadata under `resilience.*`.

This implementation also adopts the governing rules and relevant skills from `/home/morgan/dev/src/agent-toolkit-bundle`: test-first delivery, Rust quality gates, no secret or raw PII leakage, structured logging only, OTel spans/metrics for new actions and services, explicit smoke-test feedback loops, and security/compliance review for AI/MCP surfaces.

The agentic module must not depend on `rs-llmctl`. That project is standalone at `/home/morgan/dev/src/rs-llmctl` and must not be required for Tumult workspace builds, smoke tests, or agentic fault injection. Any existing local `tumult-intelligence` integration that references `rs-llmctl-client` must be detached, gated behind an optional feature, or removed from the default workspace before this change can be considered buildable.

## Goals / Non-Goals

**Goals:**

- Add a first-class agentic AI fault injection capability without replacing Tumult's existing experiment runner.
- Support an MVP target surface for HTTP agents and MCP tools/servers.
- Model GenAI-specific faults across model calls, tool calls, retrieval/context, output shape, rate limits, and retry/cost pressure.
- Evaluate behavioral contracts across a matrix of prompts/scenarios and injected faults.
- Replay captured production or test sessions as deterministic regression experiments.
- Correlate `resilience.*` experiment spans with OTel `gen_ai.*` model, workflow, tool, metric, exception, and evaluation telemetry.
- Produce TOON journals and analytics data suitable for CI gates, trend analysis, and compliance evidence.
- Gate every implementation slice with deterministic smoke tests that give clear local feedback before full workspace checks.

**Non-Goals:**

- Do not build a managed cloud service, hosted marketplace, or team dashboard in the first implementation.
- Do not depend on a specific commercial LLM provider.
- Do not require users to expose prompt/completion content by default; content capture must be explicit.
- Do not implement every framework adapter in the MVP. LangChain, AutoGen, CrewAI, and Google ADK are follow-on adapters after HTTP and MCP are solid.
- Do not replace existing infra chaos plugins. Agentic faults must compose with them.
- Do not make `rs-llmctl` a Tumult workspace dependency or an agentic module dependency.

## Decisions

### 1. Add a Native `tumult-agentic` Crate

Create a new workspace crate for agent targets, fault definitions, contract checks, replay fixtures, scoring, and telemetry helpers.

Alternatives considered:
- Script plugin only: fast to prototype, but poor fit for typed replay/session models and OTel GenAI correlation.
- Fold into `tumult-core`: simpler wiring, but it would bloat the generic runner with domain-specific GenAI concepts.

Rationale: a native crate keeps the core generic while giving agentic testing strong types and unit-testable behavior.

### 1a. Detach Existing `rs-llmctl` Coupling Before Building Agentic Work

Before implementing agentic features, the workspace must compile without `/home/morgan/dev/src/rs-llmctl` or `../rs-llmctl/rs-llmctl-client`. If `tumult-intelligence` remains in the repository, it should either:

- avoid a direct path dependency on `rs-llmctl-client`,
- move the integration behind an explicitly named optional feature that is disabled by default, or
- be excluded from the default workspace until its external dependency is made optional.

Alternatives considered:
- Require developers to clone `rs-llmctl` beside Tumult: rejected because it makes an independent repository a hidden prerequisite for Tumult builds and smoke tests.
- Reuse `rs-llmctl` as the agentic runtime: rejected because the agentic module needs provider/framework-neutral fault injection and deterministic local smoke tests.

Rationale: Tumult must remain self-contained; agentic smoke tests and CI gates must run without external project checkouts.

### 2. Use Existing Experiment Lifecycle With an Agentic Extension Section

Represent agentic tests as Tumult experiments with additional TOON sections:

```toon
agent:
  target:
    type: http
    endpoint: http://localhost:8080/invoke
  scenarios:
    - name: support-order-lookup
      input: I need help with order 12345
      expected_behavior: graceful_degradation
  faults:
    - type: model_latency
      latency_ms: 1500
      probability: 0.25
  contracts:
    - type: valid_json
    - type: retry_budget
      max_retries: 2
```

Alternatives considered:
- Introduce a separate file format: clearer domain-specific syntax, but weakens the existing TOON/journal story.
- Encode everything as generic plugin arguments: less schema work, but hard to validate and analyze.

Rationale: keep the existing five-phase evidence model and add typed agentic sections that can be validated before execution.

### 3. Start With Proxy/Adapter Fault Injection

The MVP fault engine should wrap agent dependencies at the boundaries Tumult can control:

- HTTP target adapter invokes agent endpoints and injects request/response faults around model/tool mock endpoints where configured.
- MCP target adapter invokes MCP tools and can inject faults into tool results, latency, and errors.
- Replay adapter feeds recorded model/tool/retrieval responses back to the agent deterministically.

Alternatives considered:
- Deep monkey-patching of framework internals: powerful, but language-specific and brittle.
- Network-only injection: easy for latency/errors, but cannot model malformed JSON, hallucinated tool calls, or context poisoning well.

Rationale: boundary adapters are portable, deterministic, and can later be paired with framework-specific adapters.

### 4. Contracts Are First-Class Probes

Behavioral contracts should be implemented as deterministic or evaluator-backed probes:

- deterministic: valid JSON, contains/regex, latency, retry budget, max tool calls, max tokens, no tool error leakage
- safety: no PII, no secret leakage, refusal required, citation required
- semantic/evaluator-backed: task success, factuality, citation correctness, retrieval relevance

Alternatives considered:
- Treat contracts as post-processing only: simpler, but loses per-contract span events and phase correlation.

Rationale: contracts need trace IDs, phase placement, pass/fail evidence, and scoring weight.

### 5. Scoring Uses a Contract x Fault Matrix

The agent resilience score should be a weighted aggregate over scenario, fault, and contract outcomes:

```text
score = weighted_passed_contracts / weighted_total_contracts
```

Additional sub-scores should track recovery, cost, retry behavior, latency, and replay regression.

Alternatives considered:
- Single pass/fail result: good for CI but too coarse for trend analysis.
- LLM judge-only scoring: broad but non-deterministic and costly.

Rationale: a matrix aligns with chaos engineering evidence and lets CI gates fail on specific regressions.

### 6. OTel GenAI Correlation, Not Replacement

Tumult should emit/record both namespaces:

- `resilience.*`: experiment ID, phase, fault type, scenario, contract, score, target
- `gen_ai.*`: operation name, provider, model, tool name, agent/workflow name, token metrics, operation duration, exceptions, evaluation events

The `resilience.experiment` span remains the root for Tumult-run experiments. Agent, workflow, model, retrieval, and tool spans should be linked as children where trace context can be propagated; otherwise, journals must record trace/span IDs for post-hoc correlation.

Alternatives considered:
- Invent a Tumult-only agent telemetry vocabulary: stable but isolates users from OTel GenAI tooling.
- Use only `gen_ai.*`: loses Tumult's resilience-specific experiment semantics.

Rationale: dual namespace gives compatibility and preserves Tumult's existing analytics model.

### 7. Replay Is a Data Model, Not a Log Scraper

Replay inputs should normalize imported sessions into a Tumult-owned replay format:

```toon
replay:
  source: langsmith
  session_id: abc123
  steps:
    - type: model_response
      operation: chat
      output_ref: fixtures/session-abc123/step-1.json
    - type: tool_result
      tool_name: lookup_order
      output_ref: fixtures/session-abc123/step-2.json
```

Alternatives considered:
- Directly replay vendor trace formats: faster import, but locks execution to external schemas.

Rationale: normalized replay fixtures make regression tests deterministic and portable.

### 8. Smoke Tests Gate Every Slice

Each implementation slice must add or update a fast smoke test that runs without external LLM providers or network dependencies. Smoke tests should use local fake HTTP agents, fake MCP tools, and replay fixtures.

Smoke output must name:

- adapter exercised
- scenario and fault type
- contracts evaluated
- journal path or fixture path
- trace/span assertions
- exact expected vs actual value on failure
- next diagnostic command

Example target command shape:

```bash
cargo test -p tumult-agentic smoke_ -- --nocapture
tumult agentic smoke --scenario malformed-json --target examples/agents/fake-http.toon
```

Alternatives considered:
- Rely on full `cargo test --workspace`: necessary before merge, but too slow and broad for tight feedback while building.
- Only unit-test internals: fast, but does not prove the user-visible loop from target to fault to contract to journal.

Rationale: this follows the bundle's verification-loop discipline and makes the new module practical to build incrementally.

### 9. Apply Toolkit Skill Rules During Implementation

Implementation tasks should apply these bundle skills as constraints:

- Rust/TDD: write failing tests before production code; no `.unwrap()` in library code; `thiserror` in libraries; `anyhow` only in CLI; async network calls require timeouts; `cargo fmt`, `cargo clippy`, `cargo test`, and `cargo audit` are quality gates.
- AI developer: set explicit token budgets in examples; validate structured LLM/agent output before use; validate MCP tool inputs; provide deterministic eval/replay paths.
- Chaos engineer: define steady-state hypotheses, abort conditions, rollback/reset hooks, blast radius, and production-realistic faults.
- Observability: emit OTel spans/metrics for new actions/probes, propagate trace context, use structured logging, and avoid credentials or raw sensitive content in attributes.
- Security/compliance: enforce target allowlists, metadata-only default capture, redaction, no secrets in config, structured audit events, and OWASP MCP checks.
- CI/CD: fail fast with smoke, unit, lint, security, then full workspace checks; smoke tests must be suitable for pull-request gating.

## Risks / Trade-offs

- OTel GenAI conventions are still development status -> isolate attribute constants in `tumult-agentic::telemetry` and document semconv version/opt-in behavior.
- Existing local `tumult-intelligence` changes can break workspace builds through `rs-llmctl-client` -> detach or feature-gate that integration before relying on Cargo verification.
- Fault injection can leak sensitive prompt or output content into journals -> default to metadata-only capture; require explicit content capture and redaction controls.
- Framework adapters can become a maintenance burden -> start with HTTP and MCP, then add adapters only when the boundary is stable.
- LLM-backed evaluators can be flaky and expensive -> deterministic contracts are the default; evaluator-backed checks are opt-in and separately scored.
- Agent side effects can be dangerous during replay or chaos -> provide dry-run guidance, reset hooks, idempotency warnings, and target allowlists.
- Combining infra and agent faults can create confusing failures -> require trace correlation and report the active fault timeline per scenario.
- Smoke tests can become superficial demos -> require assertions over adapter behavior, injected fault, contract result, journal fields, and trace metadata for each smoke.

## Migration Plan

1. Detach or gate the existing `rs-llmctl` / `tumult-intelligence` coupling so Tumult builds without the standalone `rs-llmctl` checkout.
2. Add `tumult-agentic` as an optional workspace crate and wire it into the CLI behind new subcommands.
3. Establish the smoke harness first: local fake HTTP agent, fake MCP tool, replay fixture, and one smoke command/test with intentionally clear failure output.
4. Implement metadata-only telemetry and deterministic contract checks before any evaluator-backed behavior.
5. Add HTTP and MCP target adapters with local fake-agent tests.
6. Add scenario pack examples and docs without changing existing experiment behavior.
7. Add analytics ingestion for agentic result rows after journal schema is stable.
8. Add framework adapters and replay importers incrementally.

Rollback is simple for the MVP: remove or disable the new crate/subcommands. Existing Tumult experiment execution remains unchanged.

## Open Questions

- Should agentic tests run through `tumult run` as normal experiments, or only through `tumult agentic run` until the schema stabilizes?
- Which replay importer should be first: LangSmith, Langfuse, OpenTelemetry trace export, or Tumult MCP trace capture?
- Should evaluator-backed semantic checks use local embeddings/LLMs first, or provider-backed models with BYOK?
- How much of the OpenTelemetry GenAI content event model should Tumult expose initially, given privacy risk?
