## Why

Tumult currently lets AI agents orchestrate chaos experiments, but it does not treat agentic AI systems themselves as resilience targets. Agentic systems depend on model providers, tools, retrieval, memory, context windows, and multi-step orchestration; those dependencies fail in ways traditional infrastructure chaos and LLM eval tools do not exercise together.

OpenTelemetry now defines GenAI semantic conventions for model calls, agent/workflow spans, tool execution, metrics, exceptions, and evaluation events. Tumult can use those conventions with its existing `resilience.*` experiment model to provide an OpenTelemetry-native fault injection and tracing module for production AI agents.

## What Changes

- Add an agentic AI fault injection module, tentatively named `tumult-agentic`, for testing agent behavior under model, tool, retrieval, context, and output faults.
- Add an agent target model that can run against HTTP agents, MCP tools/servers, and eventually framework adapters such as LangChain, AutoGen, CrewAI, and Google ADK.
- Add fault types for model latency/timeouts/rate limits, malformed or corrupted outputs, hallucinated tool calls, tool latency/failure, retrieval poisoning, context truncation, token budget exhaustion, and retry-loop pressure.
- Add behavioral contracts and probes for validity, safety, fallback behavior, citation presence, schema conformance, retry budget, task success, latency, and cost controls.
- Add deterministic replay support that can turn captured agent traces or production sessions into regression experiments.
- Add scenario packs inspired by observed market needs: concurrency storm, hallucination under timeout, cost explosion detector, malformed JSON recovery, tool timeout fallback, and retrieval poisoning.
- Correlate Tumult `resilience.*` spans and journals with OpenTelemetry `gen_ai.*` spans, metrics, exceptions, and evaluation events.
- Emit structured TOON journals and analytics rows for agent resilience scores, contract matrices, replay results, and trace correlation.

## Capabilities

### New Capabilities

- `agentic-fault-injection`: Defines how Tumult targets agentic AI systems, injects GenAI-specific faults, evaluates behavioral contracts, replays failures, and records OpenTelemetry-native evidence.

### Modified Capabilities

- None. No OpenSpec baseline capabilities exist yet; integration with existing Tumult runner, telemetry, analytics, and MCP modules is covered as implementation impact.

## Impact

- New Rust crate: `tumult-agentic` for fault models, target adapters, contract checks, replay fixtures, scoring, and telemetry helpers.
- CLI impact: new commands such as `tumult agentic run`, `tumult agentic replay`, `tumult agentic scenarios`, or equivalent subcommands.
- MCP impact: optional new MCP tools for discovering and running agentic fault scenarios.
- Experiment format impact: new TOON sections for agent targets, agent faults, prompts/scenarios, contracts, replay sources, and GenAI telemetry correlation.
- Telemetry impact: add `gen_ai.*` span attributes/events where applicable and retain `resilience.*` metadata for experiment/fault context.
- Analytics impact: add tables or views for agent runs, contract checks, fault injections, replay sessions, GenAI metrics, and resilience scores.
- Documentation impact: README positioning, agentic quickstart, OpenTelemetry GenAI observability guide, scenario pack reference, and examples.
