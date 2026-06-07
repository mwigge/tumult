---
title: Agentic Observability
parent: Guides
nav_order: 9
---

# Agentic Observability

Agentic fault injection extends Tumult's existing OpenTelemetry model with
GenAI-aware spans and metadata-only evidence. The goal is to answer three
questions after every run:

- Which scenario, fault, and contract produced the result?
- Which agent/model/tool spans correlate to the experiment?
- Did the evidence avoid storing raw prompts, completions, tool payloads, or
  retrieved documents?

## Span Shape

Agentic runs should correlate with the normal experiment root span:

```text
resilience.experiment
├── resilience.agent.scenario
│   ├── gen_ai.invoke_agent
│   ├── gen_ai.chat
│   ├── gen_ai.execute_tool
│   └── gen_ai.evaluate
└── resilience.analytics.ingest
```

The current local smoke path records deterministic trace metadata and contract
evidence without exporting spans. Production adapters should propagate
`traceparent` to HTTP or MCP targets and record returned trace/span IDs when an
agent produces them.

## Attribute Reference

Tumult keeps low-cardinality resilience attributes for experiment analysis and
adds GenAI attributes for model/tool correlation.

| Attribute | Purpose |
|---|---|
| `resilience.experiment.id` | Stable experiment identifier |
| `resilience.run.id` | Run identifier when present |
| `resilience.trace.id` | Correlated trace identifier |
| `resilience.span.id` | Correlated span identifier |
| `resilience.parent_span.id` | Parent span identifier when linked |
| `resilience.agent.scenario` | Scenario name or pack entry |
| `resilience.agent.fault.type` | Low-cardinality fault type |
| `resilience.agent.contract` | Contract name when recording an evaluation |
| `resilience.agent.score` | Run or matrix resilience score |
| `resilience.agent.payload.capture_policy` | `metadata_only`, `redacted_content`, or `raw_content` |
| `resilience.agent.input.bytes` | Prompt/input byte count |
| `resilience.agent.output.bytes` | Completion/output byte count |
| `resilience.agent.payload.sha256` | Hash for correlation without raw content |
| `gen_ai.operation.name` | `chat`, `generate_content`, `execute_tool`, `invoke_agent`, `invoke_workflow`, `evaluate`, or `embeddings` |
| `gen_ai.system` | Provider or AI system label |
| `gen_ai.request.model` | Requested model |
| `gen_ai.response.model` | Actual response model when known |
| `gen_ai.tool.name` | Tool name for tool-call spans |
| `gen_ai.evaluation.result` | Contract/evaluation label such as `pass`, `fail`, or a score bucket |

Keep scenario, fault, contract, and evaluation labels bounded. Put detailed
diagnostics in the journal, not in high-cardinality span attributes.

## Privacy Defaults

The default capture policy is `metadata_only`. Journals and spans should store:

- token counts, byte counts, hashes, scenario names, fault labels, contract
  outcomes, scores, trace IDs, and replay IDs
- no raw prompts, completions, tool inputs, tool outputs, retrieval documents,
  PII, secrets, or provider credentials

Use `redacted_content` or `raw_content` only for controlled test environments
where the target allowlist and data handling rules are explicit.

## Smoke Trace Loop

Use the smoke command before broader tests:

```bash
tumult agentic smoke
```

The output names the adapter, target type, scenario, injected fault, expected
contract result, actual contract result, resilience score, trace identifier, and
journal path, analytics ingestion result, trace assertion summary, and next
diagnostic command. A passing smoke run confirms that the fault was observed and
the contract feedback loop is readable.

## Analytics Tables

Agentic runs can be queried from the embedded analytics store:

```sql
SELECT run_id, scenario, resilience_score FROM agentic_runs;
SELECT run_id, contract_type, passed, reason FROM agentic_contract_outcomes;
SELECT run_id, fault_type, applied FROM agentic_fault_applications;
SELECT run_id, replay_id, passed FROM agentic_replay_outcomes;
```

These tables store low-cardinality labels, scores, trace IDs, and replay IDs.
They do not store raw prompts, completions, tool payloads, or retrieved
documents.
